//! cmux as a terminal host (macOS): open a session as a named, colored **cmux
//! workspace**, and later find, select and close that workspace again.
//!
//! Everything goes through cmux's own bundled CLI (`tools::tokio_command("cmux")`)
//! with an **argument list**, never a shell string. The session command line
//! is handed over as `--command`, which cmux types into the new terminal's
//! login shell, so the session inherits the user's full ambient environment;
//! nothing is injected (no `--env`/`--env-file`).
//!
//! The CLI talks to cmux over its socket, which admits an outside process like
//! mAIestro Code only in cmux's `automation` or `password` socket control mode
//! (*Settings → Automation → Socket Control Mode*). We never pass a password:
//! in password mode the CLI finds the one saved in cmux by itself. The default
//! `cmuxOnly` mode refuses us, which surfaces as an actionable error naming that
//! setting. cmux's settings, `~/.config/cmux/` and its password are **never
//! modified**.
//!
//! A stored handle is the workspace's and window's **UUIDs** — never refs or
//! indexes, which shift as windows open and close — and the workspace is looked
//! up afresh on every use, so a workspace the user dragged to another window
//! is followed.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::editor::WindowClose;
use crate::terminal_host::TerminalLayout;

/// Which cmux workspace a session runs in, and the window that held it when
/// last seen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CmuxWorkspace {
    pub workspace_id: String,
    pub window_id: String,
}

/// The settings path a user changes to let mAIestro Code control cmux.
const SOCKET_MODE_FIX: &str =
    "In cmux, open Settings → Automation and set Socket Control Mode to Automation or Password, then try again.";

/// A failed cmux CLI call, classified from its error output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CmuxError {
    /// No live cmux socket: cmux isn't running, or its socket is off.
    NotRunning,
    /// The socket refuses outside processes (`cmuxOnly`, the default).
    AccessDenied,
    /// Password mode, but the CLI found no password to present.
    PasswordMissing,
    /// Password mode, and the password the CLI presented was rejected.
    PasswordRejected,
    /// The workspace or window named no longer exists.
    NotFound,
    /// Anything else, with cmux's own words.
    Other(String),
}

impl CmuxError {
    /// The message shown to the user.
    pub fn message(&self) -> String {
        match self {
            CmuxError::NotRunning => format!("cmux isn't running, or its automation socket is off. {SOCKET_MODE_FIX}"),
            CmuxError::AccessDenied => format!("cmux doesn't let mAIestro Code control it. {SOCKET_MODE_FIX}"),
            CmuxError::PasswordMissing => "cmux's socket is in Password mode, but no password is saved in cmux. Set one \
                 in cmux Settings → Automation, then try again."
                .into(),
            CmuxError::PasswordRejected => "cmux rejected its socket password. Set it again in cmux Settings → \
                 Automation, then try again."
                .into(),
            CmuxError::NotFound => "The cmux workspace no longer exists.".into(),
            CmuxError::Other(e) => format!("cmux couldn't do that: {e}"),
        }
    }

    /// Classify the CLI's stderr (`Error: …`).
    fn classify(stderr: &str) -> Self {
        let s = stderr.trim();
        let lower = s.to_ascii_lowercase();
        if lower.contains("no live cmux socket")
            || lower.contains("socket not found")
            || lower.contains("failed to connect")
            || lower.contains("connection refused")
        {
            CmuxError::NotRunning
        } else if lower.contains("access denied") {
            CmuxError::AccessDenied
        } else if lower.contains("invalid password") {
            CmuxError::PasswordRejected
        } else if lower.contains("authentication required") || lower.contains("auth_required") {
            CmuxError::PasswordMissing
        } else if lower.contains("not_found") || lower.contains("not found") {
            CmuxError::NotFound
        } else {
            CmuxError::Other(crate::tools::snippet(s))
        }
    }
}

impl std::fmt::Display for CmuxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

/// How long one CLI call may run.
const CALL_TIMEOUT: Duration = Duration::from_secs(15);

/// How long to wait for cmux's socket after starting the app.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);

/// Open the session as a new workspace running `command` in `work_dir`, named
/// `title` and colored `color`, in the window `layout` picks (`siblings` are
/// the live handles of the sessions that layout may share a window with), and
/// return its handle. Starts cmux first when it isn't running.
pub async fn launch(
    work_dir: &Path,
    command: &str,
    title: &str,
    color: &str,
    layout: TerminalLayout,
    siblings: &[CmuxWorkspace],
) -> Result<CmuxWorkspace, String> {
    ensure_running().await.map_err(|e| e.message())?;

    let shared = match layout {
        TerminalLayout::Windows => None,
        TerminalLayout::PerRepo | TerminalLayout::Tabs => live_window(siblings).await,
    };
    // A fresh window comes with an empty workspace of its own; it is closed once
    // the session's workspace is in, so the window holds just the session.
    let (window_id, placeholder) = match shared {
        Some(w) => (w, None),
        None => {
            let w = new_window().await.map_err(|e| e.message())?;
            let placeholder = selected_workspace(&w).await;
            (w, placeholder)
        }
    };

    let out = run(&[
        "--json",
        "--id-format",
        "uuids",
        "workspace",
        "create",
        "--window",
        &window_id,
        "--name",
        title,
        "--cwd",
        &work_dir.to_string_lossy(),
        "--command",
        command,
        "--focus",
        "true",
    ])
    .await
    .map_err(|e| e.message())?;
    let workspace_id =
        parse_created(&out).ok_or_else(|| format!("cmux created a workspace but didn't report it: {}", crate::tools::snippet(&out)))?;
    let handle = CmuxWorkspace { workspace_id, window_id };

    if let Some(p) = placeholder.filter(|p| *p != handle.workspace_id) {
        if let Err(e) = run(&["workspace", "close", "--workspace", &p, "--force"]).await {
            tracing::warn!(error = %e, "could not close the new cmux window's empty workspace");
        }
    }
    if let Err(e) =
        run(&["workspace-action", "--workspace", &handle.workspace_id, "--action", "set-color", "--color", color]).await
    {
        tracing::warn!(error = %e, color, "could not color the cmux workspace");
    }
    if let Err(e) = select(&handle).await {
        tracing::warn!(error = %e, "could not bring the cmux workspace forward");
    }
    Ok(handle)
}

/// Open a plain cmux workspace (no agent) in `dir` — the repo row's "open the
/// cloned repo" button. Goes through Launch Services, like a Finder drop on
/// cmux, so it needs no socket access and leaves nothing to track.
pub fn open_folder(dir: &Path) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("cmux is only available on macOS".into());
    }
    crate::tools::spawn_reaped(std::process::Command::new("open").args(["-a", "cmux"]).arg(dir))
        .map_err(|e| format!("failed to open cmux: {e}"))
}

/// Where the session's workspace is now: `Ok(Some)` with its current window,
/// `Ok(None)` when it is gone (closed, or cmux isn't running). Never starts
/// cmux.
pub async fn locate(handle: &CmuxWorkspace) -> Result<Option<CmuxWorkspace>, CmuxError> {
    match run(&["--json", "--id-format", "uuids", "identify", "--workspace", &handle.workspace_id]).await {
        Ok(out) => Ok(parse_identified(&out, &handle.workspace_id)),
        Err(CmuxError::NotFound | CmuxError::NotRunning) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Bring the session's workspace forward: its window to the front, the
/// workspace selected in it. `Ok(None)` when it is gone, so the caller can open
/// a new one; otherwise the handle as it is now (its window may have changed).
pub async fn focus(handle: &CmuxWorkspace) -> Result<Option<CmuxWorkspace>, CmuxError> {
    let Some(current) = locate(handle).await? else {
        return Ok(None);
    };
    select(&current).await?;
    Ok(Some(current))
}

/// Close the session's workspace and confirm it is gone. `--force` because
/// cmux otherwise refuses while a process (the agent) still runs in it; the
/// user already confirmed the teardown. Without socket access we can neither
/// look nor close; then, as for VS Code, whatever still runs inside the
/// worktree decides.
pub async fn close_and_wait(handle: &CmuxWorkspace, work_dir: &Path) -> WindowClose {
    match locate(handle).await {
        Ok(None) => return WindowClose::Closed,
        Ok(Some(_)) => {}
        Err(e) => {
            tracing::warn!(error = %e, "could not look up the cmux workspace");
            return if crate::editor::worktree_in_use(work_dir).await { WindowClose::InUse } else { WindowClose::Closed };
        }
    }
    match run(&["workspace", "close", "--workspace", &handle.workspace_id, "--force"]).await {
        Ok(_) | Err(CmuxError::NotFound) => {}
        Err(e) => {
            tracing::warn!(error = %e, "could not close the cmux workspace");
            return WindowClose::StillOpen;
        }
    }
    let mut waited = 0u64;
    loop {
        match locate(handle).await {
            Ok(None) => return WindowClose::Closed,
            Ok(Some(_)) if waited >= 4000 => return WindowClose::StillOpen,
            Ok(Some(_)) => {}
            Err(_) => return WindowClose::StillOpen,
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        waited += 300;
    }
}

/// cmux's socket control mode (`access_mode`, e.g. `automation`), for Check
/// Health. Never starts cmux: `Err(NotRunning)` when it isn't.
pub async fn access_mode() -> Result<String, CmuxError> {
    let out = run(&["--json", "capabilities"]).await?;
    let v: serde_json::Value = serde_json::from_str(out.trim())
        .map_err(|_| CmuxError::Other(format!("unreadable capabilities: {}", crate::tools::snippet(&out))))?;
    Ok(v["access_mode"].as_str().unwrap_or("unknown").to_string())
}

/// Start cmux (through Launch Services) when its socket isn't up, and wait for
/// the socket. An access or password problem is returned at once.
async fn ensure_running() -> Result<(), CmuxError> {
    match run(&["ping"]).await {
        Ok(_) => return Ok(()),
        Err(CmuxError::NotRunning) => {}
        Err(e) => return Err(e),
    }
    tracing::info!("cmux isn't running; starting it");
    crate::tools::spawn_reaped(std::process::Command::new("open").args(["-a", "cmux"]))
        .map_err(|e| CmuxError::Other(format!("failed to start cmux: {e}")))?;
    let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;
    loop {
        tokio::time::sleep(Duration::from_millis(400)).await;
        match run(&["ping"]).await {
            Ok(_) => return Ok(()),
            Err(CmuxError::NotRunning) if tokio::time::Instant::now() < deadline => {}
            Err(e) => return Err(e),
        }
    }
}

/// The current window of the first sibling workspace that is still open.
async fn live_window(siblings: &[CmuxWorkspace]) -> Option<String> {
    for s in siblings {
        if let Ok(Some(current)) = locate(s).await {
            return Some(current.window_id);
        }
    }
    None
}

/// Open a new cmux window and return its UUID (`OK <uuid>`).
async fn new_window() -> Result<String, CmuxError> {
    let out = run(&["new-window"]).await?;
    parse_ok_id(&out).ok_or_else(|| CmuxError::Other(format!("new-window didn't report the window: {}", crate::tools::snippet(&out))))
}

/// The workspace selected in `window`, if any.
async fn selected_workspace(window: &str) -> Option<String> {
    let out = run(&["--json", "--id-format", "uuids", "identify", "--window", window, "--no-caller"]).await.ok()?;
    let v: serde_json::Value = serde_json::from_str(out.trim()).ok()?;
    v["focused"]["workspace_id"].as_str().map(str::to_string)
}

/// Raise the workspace's window and select the workspace in it.
async fn select(handle: &CmuxWorkspace) -> Result<(), CmuxError> {
    run(&["focus-window", "--window", &handle.window_id]).await?;
    run(&["workspace", "select", "--workspace", &handle.workspace_id, "--window", &handle.window_id]).await?;
    Ok(())
}

/// The new workspace's UUID from `workspace create --json --id-format uuids`.
fn parse_created(out: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(out.trim()).ok()?;
    v["workspace_id"].as_str().filter(|s| is_uuid(s)).map(str::to_string)
}

/// The workspace's handle from `identify --workspace <uuid>`: its `caller`
/// block, `null` when no workspace has that id.
fn parse_identified(out: &str, workspace_id: &str) -> Option<CmuxWorkspace> {
    let v: serde_json::Value = serde_json::from_str(out.trim()).ok()?;
    let caller = &v["caller"];
    if !caller["workspace_id"].as_str().is_some_and(|w| w.eq_ignore_ascii_case(workspace_id)) {
        return None;
    }
    let window_id = caller["window_id"].as_str().filter(|s| is_uuid(s))?;
    Some(CmuxWorkspace { workspace_id: workspace_id.to_string(), window_id: window_id.to_string() })
}

/// The UUID in a legacy `OK <uuid>` reply.
fn parse_ok_id(out: &str) -> Option<String> {
    let id = out.trim().strip_prefix("OK ")?.trim();
    is_uuid(id).then(|| id.to_string())
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| if matches!(i, 8 | 13 | 18 | 23) { b == b'-' } else { b.is_ascii_hexdigit() })
}

/// Run the cmux CLI with `args`, returning its stdout. The caller-context
/// variables cmux sets in its own terminals are dropped, so a mAIestro Code
/// started from a cmux terminal (in development) never targets its own
/// workspace by accident; nothing is added.
async fn run(args: &[&str]) -> Result<String, CmuxError> {
    let mut cmd = crate::tools::tokio_command("cmux");
    cmd.args(args)
        .env("CMUX_QUIET", "1")
        .env_remove("CMUX_WORKSPACE_ID")
        .env_remove("CMUX_SURFACE_ID")
        .env_remove("CMUX_TAB_ID")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let out = match tokio::time::timeout(CALL_TIMEOUT, cmd.output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return Err(CmuxError::Other(format!("couldn't run the cmux CLI: {e}"))),
        Err(_) => return Err(CmuxError::Other("cmux didn't respond".into())),
    };
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Some failures are reported on stdout (`ERROR: …`).
    Err(CmuxError::classify(if stderr.trim().is_empty() { &stdout } else { &stderr }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WS: &str = "6F1B2C3D-4E5F-4A6B-8C7D-9E0F1A2B3C4D";
    const WIN: &str = "A1B2C3D4-E5F6-4A7B-8C9D-0E1F2A3B4C5D";

    #[test]
    fn classifies_the_clis_errors() {
        let no_socket = "Error: No live cmux socket found. Tried:\n  /Users/u/.local/state/cmux/cmux.sock\n";
        assert_eq!(CmuxError::classify(no_socket), CmuxError::NotRunning);
        assert_eq!(
            CmuxError::classify("Error: Access denied - only processes started inside cmux can connect"),
            CmuxError::AccessDenied
        );
        assert_eq!(CmuxError::classify("ERROR: Invalid password"), CmuxError::PasswordRejected);
        assert_eq!(
            CmuxError::classify("ERROR: Authentication required. Send auth <password> first."),
            CmuxError::PasswordMissing
        );
        assert_eq!(CmuxError::classify("Error: not_found: Workspace not found"), CmuxError::NotFound);
        assert!(matches!(CmuxError::classify("Error: something odd"), CmuxError::Other(_)));
    }

    /// Every user-facing access error names the setting that fixes it.
    #[test]
    fn access_errors_name_the_fix() {
        for e in [CmuxError::NotRunning, CmuxError::AccessDenied] {
            assert!(e.message().contains("Socket Control Mode"), "{e}");
            assert!(e.message().contains("Automation or Password"), "{e}");
        }
        assert!(CmuxError::PasswordMissing.message().contains("Password mode"));
        assert!(CmuxError::PasswordRejected.message().contains("rejected"));
    }

    #[test]
    fn parses_the_created_workspace() {
        let out = format!(r#"{{"workspace_id":"{WS}","window_id":"{WIN}"}}"#);
        assert_eq!(parse_created(&out).as_deref(), Some(WS));
        assert_eq!(parse_created(r#"{"workspace_ref":"workspace:3"}"#), None);
        assert_eq!(parse_created("OK workspace:3"), None);
    }

    #[test]
    fn parses_an_identified_workspace() {
        let out = format!(
            r#"{{"socket_path":"/s","focused":null,"caller":{{"workspace_id":"{WS}","window_id":"{WIN}","surface_id":null}}}}"#
        );
        assert_eq!(
            parse_identified(&out, WS),
            Some(CmuxWorkspace { workspace_id: WS.into(), window_id: WIN.into() })
        );
        assert_eq!(parse_identified(&out, &WS.to_lowercase()).map(|h| h.window_id), Some(WIN.to_string()));
        assert_eq!(parse_identified(r#"{"socket_path":"/s","focused":null,"caller":null}"#, WS), None);
        assert_eq!(parse_identified("garbage", WS), None);
    }

    #[test]
    fn parses_the_new_windows_id() {
        assert_eq!(parse_ok_id(&format!("OK {WIN}\n")).as_deref(), Some(WIN));
        assert_eq!(parse_ok_id("OK window:2"), None);
        assert_eq!(parse_ok_id(WIN), None);
    }

    /// End to end against the real cmux (socket mode Automation or Password):
    /// launch a colored workspace, find it, focus it, then close it with a
    /// process still running in it. Opens and closes a real workspace, so it
    /// runs only on demand: `cargo test cmux -- --ignored`.
    #[tokio::test]
    #[ignore = "drives the real cmux"]
    async fn launch_focus_and_close_a_real_workspace() {
        let dir = std::env::temp_dir();
        let handle = launch(&dir, "echo maiestro-cmux-test && sleep 600", "maiestro: cmux test", "#dc2626", TerminalLayout::Windows, &[])
            .await
            .expect("launch");
        assert_eq!(locate(&handle).await.unwrap().as_ref(), Some(&handle));
        assert!(focus(&handle).await.unwrap().is_some());
        // A second session in the same layout group shares the window.
        let sibling = launch(&dir, "sleep 600", "maiestro: cmux sibling", "#2563eb", TerminalLayout::PerRepo, std::slice::from_ref(&handle))
            .await
            .expect("sibling");
        assert_eq!(sibling.window_id, handle.window_id);
        assert_eq!(close_and_wait(&sibling, &dir).await, WindowClose::Closed);
        assert_eq!(close_and_wait(&handle, &dir).await, WindowClose::Closed);
        assert_eq!(locate(&handle).await.unwrap(), None);
    }
}
