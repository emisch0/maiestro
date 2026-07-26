//! Test-only support for hermetic filesystem tests.
//!
//! The whole stateful layer (`sessions`, `status`, `repo_settings`,
//! `app_settings`, `identities`) resolves its files through
//! [`crate::paths::maiestro_dir`], which honors the `MAIESTRO_HOME` env override
//! (see [`crate::paths::MAIESTRO_HOME_ENV`]). [`TempHome`] points that override at
//! a fresh tempdir for the duration of a test so nothing touches the developer's
//! real `~/.maiestro`.
//!
//! Because environment variables are process-global, concurrently-running tests
//! would otherwise stomp on each other's `MAIESTRO_HOME`. A single global
//! [`Mutex`] serializes every `TempHome`, so at most one env-overriding test runs
//! at a time; the rest of the suite stays parallel. The guard restores the prior
//! value (or unsets it) on drop, even on panic.

use crate::paths::MAIESTRO_HOME_ENV;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use tempfile::TempDir;

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// A scratch `MAIESTRO_HOME` active for the guard's lifetime.
///
/// Holds the serializing lock and a `TempDir` that is deleted on drop. Obtain the
/// root with [`TempHome::path`]; the config/state modules pick it up automatically
/// because they go through `maiestro_dir`.
pub struct TempHome {
    dir: TempDir,
    prev: Option<String>,
    // Held for the guard's lifetime to serialize env mutation. `'static` because
    // the lock itself is `'static`; the poison guard is intentionally ignored.
    _lock: MutexGuard<'static, ()>,
}

impl TempHome {
    /// Create a fresh tempdir and point `MAIESTRO_HOME` at it. Blocks until any
    /// other `TempHome` is dropped.
    pub fn new() -> Self {
        // Recover from a poisoned lock: a prior test panicking while holding it
        // must not wedge the rest of the suite.
        let lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("create MAIESTRO_HOME tempdir");
        let prev = std::env::var(MAIESTRO_HOME_ENV).ok();
        std::env::set_var(MAIESTRO_HOME_ENV, dir.path());
        TempHome { dir, prev, _lock: lock }
    }

    /// The tempdir standing in for `~/.maiestro`.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// A `<root>/<rel>` path inside this temp home.
    pub fn join(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(v) => std::env::set_var(MAIESTRO_HOME_ENV, v),
            None => std::env::remove_var(MAIESTRO_HOME_ENV),
        }
    }
}
