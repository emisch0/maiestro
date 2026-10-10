//! Terminal.app as a terminal host (macOS): open a session in its own window,
//! and later find, focus and close that window again.
//!
//! Everything goes through `osascript` with an **argument list**: the scripts
//! below are constants that read their values from `argv`, so nothing we pass —
//! the command line, the title — is ever spliced into AppleScript source. The
//! command line Terminal types into its new shell is quoted for that shell with
//! `tools::shell_quote`. `do script` runs in the user's login shell, so the
//! session inherits their full ambient environment; we never construct one.
//!
//! Terminal's **profiles are never modified**: no settings set is created,
//! edited or selected, and nothing is written to Terminal's preferences. The
//! title and colors are properties of the one tab we opened, and we only ever
//! close that window.
//!
//! Terminal reuses window ids once it restarts, so a stored handle is trusted
//! only while its window still holds a tab on the recorded `tty`.
//!
//! The first Apple event to Terminal raises macOS's Automation prompt ("mAIestro
//! Code wants to control Terminal"). A denial fails every script with `-1743`,
//! which surfaces as [`AUTOMATION_DENIED`].

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::editor::WindowClose;
use crate::tools::shell_quote;

/// Which Terminal window a session runs in: the window's AppleScript `id` and
/// the `tty` of the session's tab (e.g. `/dev/ttys004`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalAppWindow {
    pub window_id: i64,
    pub tty: String,
}

/// The message for a denied Automation grant, naming where to fix it.
pub const AUTOMATION_DENIED: &str = "mAIestro Code isn't allowed to control Terminal. Allow it in System Settings → \
     Privacy & Security → Automation → mAIestro Code → Terminal, then try again.";

/// Near-white, in AppleScript's 16-bit channels, for the session tab's text:
/// readable on every [`crate::theming::dark_tint`] background whatever the
/// user's profile draws text in.
const TEXT_COLOR: [u32; 3] = [0xf0f0, 0xf0f0, 0xf0f0];

/// How long one script may run. Generous because the first Apple event waits
/// while the user answers the Automation prompt.
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(120);

/// Open a new window running `command` in `work_dir`, titled `title` and tinted
/// with a dark version of `color`, and return its handle.
///
/// When Terminal isn't running, launching it opens its usual startup window;
/// the session then runs in that window rather than leaving an empty one
/// beside it.
const LAUNCH_SCRIPT: &str = r#"on run argv
	set cmd to item 1 of argv
	set ttl to item 2 of argv
	set bg to {(item 3 of argv) as integer, (item 4 of argv) as integer, (item 5 of argv) as integer}
	set fg to {(item 6 of argv) as integer, (item 7 of argv) as integer, (item 8 of argv) as integer}
	set wasRunning to application "Terminal" is running
	tell application "Terminal"
		if not wasRunning then
			launch
			repeat 30 times
				if (count of windows) > 0 then exit repeat
				delay 0.1
			end repeat
		end if
		if (not wasRunning) and (count of windows) > 0 then
			set t to do script cmd in window 1
		else
			set t to do script cmd
		end if
		set custom title of t to ttl
		set title displays custom title of t to true
		if (item 9 of argv) is "tint" then
			set background color of t to bg
			set normal text color of t to fg
			set bold text color of t to fg
		end if
		set ttyName to tty of t
		set wid to missing value
		repeat with w in windows
			repeat with tb in tabs of w
				if tty of tb is ttyName then set wid to id of w
			end repeat
		end repeat
		activate
	end tell
	return (wid as text) & linefeed & ttyName
end run"#;

/// `probe`, `focus` or `close` the window `item 1` if it still holds a tab on
/// tty `item 2`. Never launches Terminal: a Terminal that isn't running has no
/// session window.
const WINDOW_SCRIPT: &str = r#"on run argv
	set wid to (item 1 of argv) as integer
	set ttyName to item 2 of argv
	set mode to item 3 of argv
	if application "Terminal" is not running then return "absent"
	tell application "Terminal"
		if not (exists window id wid) then return "absent"
		set w to window id wid
		set ours to false
		repeat with tb in tabs of w
			if tty of tb is ttyName then set ours to true
		end repeat
		if not ours then return "absent"
		if mode is "focus" then
			set miniaturized of w to false
			set index of w to 1
			activate
		else if mode is "close" then
			close w saving no
		end if
	end tell
	return "open"
end run"#;

/// Open the session in a new Terminal window. `command` is the agent command
/// line (already quoted for a POSIX shell); it runs after a `cd` into the
/// worktree and an OSC 0 that also sets the window title from the shell.
pub async fn launch(work_dir: &Path, command: &str, title: &str, color: &str) -> Result<TerminalAppWindow, String> {
    let line = format!(
        "cd {} && printf '\\033]0;%s\\007' {} && {command}",
        shell_quote(&work_dir.to_string_lossy()),
        shell_quote(title),
    );
    let tint = crate::theming::dark_tint(color);
    let bg = tint.map(|c| c.map(|v| u32::from(v) * 257)).unwrap_or([0; 3]);
    let mut args = vec![line, title.to_string()];
    args.extend(bg.iter().chain(TEXT_COLOR.iter()).map(u32::to_string));
    args.push(if tint.is_some() { "tint" } else { "plain" }.to_string());
    let out = run(LAUNCH_SCRIPT, &args).await?;
    parse_handle(&out).ok_or_else(|| format!("Terminal opened a window but didn't report it: {out:?}"))
}

/// Focus the session's window. `Ok(false)` when it is gone (closed, or Terminal
/// quit), so the caller can open a new one.
pub async fn focus(handle: &TerminalAppWindow) -> Result<bool, String> {
    window(handle, "focus").await
}

/// Whether the session's window is still open.
pub async fn probe(handle: &TerminalAppWindow) -> Result<bool, String> {
    window(handle, "probe").await
}

/// Close the session's window and confirm it is gone.
///
/// The agent is ended first — a hangup to every process on the tab's tty, as
/// closing a terminal would send — so Terminal has nothing running to ask
/// "terminate running processes?" about. That signal goes out only after the
/// probe has confirmed the tty still belongs to this window: a tty is reused
/// once its window closes, and must never take down someone else's shell.
/// Without the Automation grant we can neither look nor close; then, as for
/// VS Code, whatever still runs inside the worktree decides.
pub async fn close_and_wait(handle: &TerminalAppWindow, work_dir: &Path) -> WindowClose {
    match probe(handle).await {
        Ok(false) => return WindowClose::Closed,
        Ok(true) => {}
        Err(e) => {
            tracing::warn!(error = %e, "could not probe the Terminal window");
            return if crate::editor::worktree_in_use(work_dir).await { WindowClose::InUse } else { WindowClose::Closed };
        }
    }
    signal_tty(&handle.tty, "HUP").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut waited = 0u64;
    let mut killed = false;
    loop {
        match window(handle, "close").await {
            Ok(false) => return WindowClose::Closed,
            Ok(true) => {}
            Err(e) => {
                tracing::warn!(error = %e, "could not close the Terminal window");
                return WindowClose::StillOpen;
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        waited += 300;
        match probe(handle).await {
            Ok(false) => return WindowClose::Closed,
            Ok(true) if waited >= 4000 => return WindowClose::StillOpen,
            Ok(true) => {
                // Something ignored the hangup and Terminal is holding the
                // window open for it; the user already confirmed the teardown.
                if !killed && waited >= 1500 {
                    signal_tty(&handle.tty, "KILL").await;
                    killed = true;
                }
            }
            Err(_) => return WindowClose::StillOpen,
        }
    }
}

/// Whether mAIestro Code may send Terminal Apple events, for Check Health.
/// `Ok(None)` when Terminal isn't running: asking would launch it, and the
/// grant is requested on the first real launch anyway.
pub async fn automation_allowed() -> Result<Option<bool>, String> {
    match run(AUTOMATION_SCRIPT, &[]).await {
        Ok(out) if out.trim() == "not running" => Ok(None),
        Ok(_) => Ok(Some(true)),
        Err(e) if e == AUTOMATION_DENIED => Ok(Some(false)),
        Err(e) => Err(e),
    }
}

/// A harmless Apple event to a Terminal that is already running.
const AUTOMATION_SCRIPT: &str = r#"on run argv
	if application "Terminal" is not running then return "not running"
	tell application "Terminal" to get version
	return "ok"
end run"#;

async fn window(handle: &TerminalAppWindow, mode: &str) -> Result<bool, String> {
    let out = run(WINDOW_SCRIPT, &[handle.window_id.to_string(), handle.tty.clone(), mode.to_string()]).await?;
    Ok(out.trim() == "open")
}

/// Send `signal` to every process of ours on `tty`. The processes come from
/// `ps -t`, not `pkill -t`, which can't resolve `ttys…` names on current macOS
/// ("No such tty"). The root-owned `login` that Terminal starts the shell under
/// is never ours, so it's left alone. `ps` and `kill` are system binaries on
/// the minimal PATH, like `lsof`; a tty with nothing left on it is fine.
async fn signal_tty(tty: &str, signal: &str) {
    let (Some(name), Some(uid)) = (tty_name(tty), current_uid().await) else {
        return;
    };
    let pids = match tokio::process::Command::new("ps").args(["-t", name, "-o", "pid=,uid="]).output().await {
        Ok(out) => own_pids(&String::from_utf8_lossy(&out.stdout), &uid),
        Err(e) => {
            tracing::warn!(error = %e, tty, "could not list the Terminal session's processes");
            return;
        }
    };
    if pids.is_empty() {
        return;
    }
    match tokio::process::Command::new("kill").arg(format!("-{signal}")).args(&pids).output().await {
        Ok(out) => tracing::info!(tty, signal, processes = pids.len(), status = ?out.status.code(), "signalled the Terminal session's processes"),
        Err(e) => tracing::warn!(error = %e, tty, "could not signal the Terminal session's processes"),
    }
}

/// The pids in `ps -o pid=,uid=` output that belong to `uid`.
fn own_pids(ps_out: &str, uid: &str) -> Vec<String> {
    ps_out
        .lines()
        .filter_map(|line| {
            let mut cols = line.split_whitespace();
            let (pid, owner) = (cols.next()?, cols.next()?);
            (owner == uid && pid.bytes().all(|b| b.is_ascii_digit())).then(|| pid.to_string())
        })
        .collect()
}

/// The current user's uid, so only our own processes are ever signalled.
async fn current_uid() -> Option<String> {
    let out = tokio::process::Command::new("id").arg("-u").output().await.ok()?;
    let uid = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!uid.is_empty() && uid.bytes().all(|b| b.is_ascii_digit())).then_some(uid)
}

/// `ttys004` from `/dev/ttys004` — the form `ps -t` takes. `None` for
/// anything that isn't a plain tty device name.
fn tty_name(tty: &str) -> Option<&str> {
    let name = tty.strip_prefix("/dev/")?;
    (name.starts_with("tty") && name.len() > 3 && name.bytes().all(|b| b.is_ascii_alphanumeric())).then_some(name)
}

/// Parse the launch script's `<window id>\n<tty>`.
fn parse_handle(out: &str) -> Option<TerminalAppWindow> {
    let mut lines = out.trim().lines();
    let window_id = lines.next()?.trim().parse().ok()?;
    let tty = lines.next()?.trim().to_string();
    tty_name(&tty)?;
    Some(TerminalAppWindow { window_id, tty })
}

/// Run an AppleScript with `args` as its `argv`, returning its stdout. A denied
/// Automation grant becomes [`AUTOMATION_DENIED`].
async fn run(script: &str, args: &[String]) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new("osascript");
    cmd.arg("-e").arg(script).args(args).kill_on_drop(true);
    let out = match tokio::time::timeout(SCRIPT_TIMEOUT, cmd.output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return Err(format!("couldn't run osascript: {e}")),
        Err(_) => return Err("Terminal didn't respond".into()),
    };
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if is_automation_denied(&stderr) {
        return Err(AUTOMATION_DENIED.into());
    }
    Err(format!("Terminal couldn't do that: {}", crate::tools::snippet(stderr.trim())))
}

/// Whether osascript's error is the Automation denial (`errAEEventNotPermitted`).
fn is_automation_denied(stderr: &str) -> bool {
    stderr.contains("-1743")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_launch_scripts_handle() {
        assert_eq!(
            parse_handle("4321\n/dev/ttys004\n"),
            Some(TerminalAppWindow { window_id: 4321, tty: "/dev/ttys004".into() })
        );
        assert_eq!(parse_handle("missing value\n/dev/ttys004"), None);
        assert_eq!(parse_handle("4321"), None);
        assert_eq!(parse_handle("4321\n/etc/passwd"), None);
    }

    /// Only our own processes on the tty are signalled — never Terminal's
    /// root-owned `login`.
    #[test]
    fn own_pids_keeps_only_our_processes() {
        let ps = "28367     0\n28368   501\n28379   501\n";
        assert_eq!(own_pids(ps, "501"), ["28368", "28379"]);
        assert!(own_pids("", "501").is_empty());
        assert!(own_pids("garbage\n", "501").is_empty());
    }

    /// Only a plain tty device name ever reaches `ps -t`.
    #[test]
    fn tty_name_accepts_only_tty_devices() {
        assert_eq!(tty_name("/dev/ttys004"), Some("ttys004"));
        assert_eq!(tty_name("ttys004"), None);
        assert_eq!(tty_name("/dev/tty"), None);
        assert_eq!(tty_name("/dev/ttys004 -a"), None);
        assert_eq!(tty_name("/dev/disk0"), None);
    }

    #[test]
    fn recognizes_a_denied_automation_grant() {
        assert!(is_automation_denied(
            "execution error: Not authorized to send Apple events to Terminal. (-1743)"
        ));
        assert!(!is_automation_denied("execution error: Terminal got an error: Can’t get window id 3. (-1728)"));
    }

    /// End to end against the real Terminal.app: open a tinted window, check
    /// that only the tab — not its profile — changed, focus it, then close it
    /// with a process still running in it. Opens and closes a real window, so
    /// it runs only on demand: `cargo test terminal -- --ignored`.
    #[tokio::test]
    #[ignore = "drives the real Terminal.app"]
    async fn launch_focus_and_close_a_real_window() {
        let profiles = || {
            std::process::Command::new("defaults")
                .args(["read", "com.apple.Terminal", "Window Settings"])
                .output()
                .unwrap()
                .stdout
        };
        let before = profiles();
        let dir = std::env::temp_dir();
        let handle = launch(&dir, "echo maiestro-terminal-test && sleep 600", "maiestro: terminal test", "#dc2626")
            .await
            .expect("launch");
        assert!(probe(&handle).await.unwrap(), "the new window is open");
        let colors = run(
            r#"on run argv
	tell application "Terminal"
		set t to first tab of window id ((item 1 of argv) as integer)
		set bg to background color of t
		set profileBg to background color of settings set (name of current settings of t)
		return ((item 1 of bg) as text) & "," & ((item 1 of profileBg) as text)
	end tell
end run"#,
            &[handle.window_id.to_string()],
        )
        .await
        .unwrap();
        let (tab, profile) = colors.trim().split_once(',').unwrap();
        assert_eq!(tab, (41u32 * 257).to_string(), "the tab carries the tint");
        assert_ne!(tab, profile, "the profile keeps its own background");
        assert!(focus(&handle).await.unwrap());
        assert!(matches!(close_and_wait(&handle, &dir).await, WindowClose::Closed));
        assert!(!probe(&handle).await.unwrap(), "the window is gone");
        assert_eq!(before, profiles(), "Terminal's profiles are unchanged");
    }

    /// The scripts take every value from `argv`: nothing we pass is spliced into
    /// their source.
    #[test]
    fn scripts_are_constant() {
        assert!(LAUNCH_SCRIPT.starts_with("on run argv"));
        assert!(WINDOW_SCRIPT.starts_with("on run argv"));
        assert!(AUTOMATION_SCRIPT.starts_with("on run argv"));
        assert!(!LAUNCH_SCRIPT.contains("{}") && !WINDOW_SCRIPT.contains("{}"));
    }
}
