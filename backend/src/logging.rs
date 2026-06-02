//! Backend logging: a `tracing` subscriber writing to a daily, UTC-dated file
//! plus stderr.
//!
//! The file path is `~/Library/Logs/com.maiestro.app/lYYYYMM/maiestro-YYYYMMDD.log`,
//! with the monthly directory and the filename both derived from the current UTC
//! date. The file rolls over at UTC midnight even while the long-running menu-bar
//! process keeps going — which is why we use a custom writer rather than
//! `tauri-plugin-log` (its path is fixed at startup and it only rotates by size).
//!
//! Logging never records credentials: command invocations log credential *types*
//! and scopes but never secret values, and the GitHub client logs request URLs
//! but never the `Authorization` header (see `plugins/github.rs`).

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use time::OffsetDateTime;
use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

/// Root directory for log files: `~/Library/Logs/com.maiestro.app`.
fn log_root() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Logs/com.maiestro.app")
}

/// A `MakeWriter` that appends to a per-day file and reopens the next day's file
/// when the UTC date changes. Cheap to clone (shares one mutex-guarded handle).
#[derive(Clone)]
pub struct DailyRollingWriter {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    root: PathBuf,
    /// The (year, month, day) and handle of the currently open file, if any.
    current: Option<(i32, u8, u8, File)>,
}

impl DailyRollingWriter {
    fn new(root: PathBuf) -> Self {
        Self { inner: Arc::new(Mutex::new(Inner { root, current: None })) }
    }
}

impl Write for DailyRollingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let now = OffsetDateTime::now_utc();
        let (y, m, d) = (now.year(), u8::from(now.month()), now.day());

        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        let needs_open = !matches!(inner.current, Some((cy, cm, cd, _)) if cy == y && cm == m && cd == d);
        if needs_open {
            let dir = inner.root.join(format!("l{y:04}{m:02}"));
            std::fs::create_dir_all(&dir)?;
            let path = dir.join(format!("maiestro-{y:04}{m:02}{d:02}.log"));
            let file = OpenOptions::new().create(true).append(true).open(path)?;
            inner.current = Some((y, m, d, file));
        }

        // Safe: just set above when it was None.
        let (_, _, _, file) = inner.current.as_mut().unwrap();
        file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match inner.current.as_mut() {
            Some((_, _, _, file)) => file.flush(),
            None => Ok(()),
        }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for DailyRollingWriter {
    type Writer = DailyRollingWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Install the global tracing subscriber. Idempotent: a second call is a no-op,
/// so tests that init repeatedly don't panic. A file-open failure on any line
/// only loses that line's file copy — the stderr layer still records it.
pub fn init() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_timer(UtcTime::rfc_3339())
        .with_writer(DailyRollingWriter::new(log_root()));

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_timer(UtcTime::rfc_3339())
        .with_writer(std::io::stderr);

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_session_field_appears_on_nested_events() {
        // A capturing writer so we can inspect the formatted output.
        #[derive(Clone)]
        struct Buf(Arc<Mutex<Vec<u8>>>);
        impl Write for Buf {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Buf {
            type Writer = Buf;
            fn make_writer(&'a self) -> Buf {
                self.clone()
            }
        }

        // Mirrors how the session_* commands are instrumented: a span carrying the
        // workspace session id, with a nested event emitted from inside it.
        #[tracing::instrument(skip_all, fields(session = %id))]
        fn session_op(id: &str) {
            tracing::info!("nested event");
        }

        let buf = Buf(Arc::new(Mutex::new(Vec::new())));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buf.clone())
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, || session_op("28-add-foo"));

        let out = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        assert!(out.contains("session=28-add-foo"), "session id missing from line: {out}");
        assert!(out.contains("nested event"), "nested event missing: {out}");
    }

    #[test]
    fn writes_to_utc_dated_path() {
        let root = std::env::temp_dir().join(format!("maiestro-log-test-{}", uuid::Uuid::new_v4()));
        let mut w = DailyRollingWriter::new(root.clone());
        w.write_all(b"hello\n").unwrap();
        w.flush().unwrap();

        let now = OffsetDateTime::now_utc();
        let (y, m, d) = (now.year(), u8::from(now.month()), now.day());
        let expected = root
            .join(format!("l{y:04}{m:02}"))
            .join(format!("maiestro-{y:04}{m:02}{d:02}.log"));

        let contents = std::fs::read_to_string(&expected).expect("log file at UTC-dated path");
        assert!(contents.contains("hello"));

        std::fs::remove_dir_all(&root).ok();
    }
}

/// Log a Tauri command invocation at `info`. Call as the first statement of a
/// `#[tauri::command]`. Pass the command name and any non-secret key args using
/// `tracing` field syntax — NEVER a credential value.
///
/// ```ignore
/// crate::log_invoke!("session_create_pr", session_id = %session_id);
/// ```
#[macro_export]
macro_rules! log_invoke {
    ($cmd:expr) => {
        ::tracing::info!(target: "invoke", command = $cmd, "command invoked")
    };
    ($cmd:expr, $($field:tt)+) => {
        ::tracing::info!(target: "invoke", command = $cmd, $($field)+, "command invoked")
    };
}
