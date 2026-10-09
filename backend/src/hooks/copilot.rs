//! Copilot's status hooks, in our own file under the worktree's `.github/hooks/`.
//!
//! Copilot reads repo hooks from `.github/hooks/*.json`, any filename, so we own
//! a whole file, `.github/hooks/maiestro-status.json` ([`copilot_hooks`]), kept
//! out of git through `info/exclude`. Copilot loads it only once the user trusts
//! the folder (its own "Do you trust the files in this folder?" prompt, shown for
//! every new worktree anyway). Silent commands are neutral there — they neither
//! approve nor deny a tool — and ours are silent and always exit 0, like
//! Antigravity's.

use std::path::{Path, PathBuf};

use super::{silent_hook_command, HookShell};
use crate::agent::Agent;

/// Our Copilot hook file, relative to the worktree (and as an `info/exclude`
/// line). A mAIestro-only filename, so unlike Antigravity's shared
/// `.agents/hooks.json` it never needs a tracked `.gitignore` line.
pub(super) const COPILOT_HOOK_REL: &str = ".github/hooks/maiestro-status.json";

/// The worktree file Copilot reads mAIestro Code's status hooks from.
pub(super) fn copilot_hook_file(work_dir: &Path) -> PathBuf {
    work_dir.join(COPILOT_HOOK_REL)
}

/// Copilot hook events → helper verbs. Copilot ignores event names it doesn't
/// know, so a missing event in an older CLI is harmless.
///
/// - `sessionStart` → `session_start`: running, unless the same Copilot session
///   is already mid-turn (it can fire *after* `userPromptSubmitted`).
/// - `permissionRequest` → `notification`: needs you, naming the tool.
/// - `postToolUseFailure` → `tool_failed`, its `error` string feeding the
///   persistent-failure logic. A denied tool fires no post-tool event, and a
///   shell command that exits non-zero still counts as `postToolUse`.
/// - `errorOccurred` → `error`: a surfaced `last_error`.
const COPILOT_HOOK_EVENTS: &[(&str, &str)] = &[
    ("sessionStart", "session_start"),
    ("userPromptSubmitted", "prompt"),
    ("preToolUse", "busy"),
    ("permissionRequest", "notification"),
    ("postToolUse", "tool_ok"),
    ("postToolUseFailure", "tool_failed"),
    ("errorOccurred", "error"),
    ("agentStop", "idle"),
    ("sessionEnd", "ended"),
];

/// mAIestro Code's Copilot hook file for a worktree, pointing at `bin`. Copilot
/// runs a handler's `bash` on Unix and its `powershell` on Windows; the
/// `powershell` form is written only on Windows, so the file elsewhere stays
/// exactly as before.
fn copilot_hooks(bin: &Path, ws_id: &str) -> serde_json::Value {
    let shell = HookShell::for_agent(Agent::Copilot);
    let hooks: serde_json::Map<String, serde_json::Value> = COPILOT_HOOK_EVENTS
        .iter()
        .map(|(event, verb)| {
            let mut handler = serde_json::json!({
                "type": "command",
                "bash": silent_hook_command(HookShell::Posix, bin, ws_id, verb),
                "timeoutSec": 10,
            });
            if shell == HookShell::PowerShell {
                handler["powershell"] = silent_hook_command(shell, bin, ws_id, verb).into();
            }
            (event.to_string(), serde_json::json!([handler]))
        })
        .collect();
    serde_json::json!({ "version": 1, "hooks": hooks })
}

fn copilot_hooks_body(bin: &Path, ws_id: &str) -> String {
    serde_json::to_string_pretty(&copilot_hooks(bin, ws_id)).unwrap() + "\n"
}

/// Write our Copilot hook file. We own it outright, so it is simply replaced.
pub(super) fn write_copilot_hooks(work_dir: &Path, ws_id: &str, bin: &Path) -> Result<(), String> {
    let path = copilot_hook_file(work_dir);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, copilot_hooks_body(bin, ws_id)).map_err(|e| e.to_string())
}

/// The Copilot half of [`reconcile_session_hooks`](super::reconcile_session_hooks) against an explicit `bin`:
/// rewrite our file only if it is already there and differs, so a worktree
/// whose hooks were removed is never re-injected.
pub(super) fn reconcile_copilot_hooks(work_dir: &Path, ws_id: &str, bin: &Path) -> bool {
    let path = copilot_hook_file(work_dir);
    let Ok(existing) = std::fs::read_to_string(&path) else {
        return false;
    };
    let updated = copilot_hooks_body(bin, ws_id);
    if updated == existing {
        return false;
    }
    std::fs::write(&path, updated).is_ok()
}

/// Delete our Copilot hook file — used when a session switches away from
/// Copilot, so a `copilot` run by hand there no longer writes the session's
/// status — and the `.github/hooks/` and `.github/` folders if that left them
/// empty. Best-effort; returns whether it removed the file.
pub fn remove_copilot_hooks(work_dir: &Path) -> bool {
    let path = copilot_hook_file(work_dir);
    if std::fs::remove_file(&path).is_err() {
        return false;
    }
    // `remove_dir` only removes an empty folder, so anything the repo keeps there stays.
    if let Some(hooks_dir) = path.parent() {
        if std::fs::remove_dir(hooks_dir).is_ok() {
            let _ = hooks_dir.parent().map(std::fs::remove_dir);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::{assert_silent_success, write_session_hooks};

    /// Copilot reads hook output as a decision for `preToolUse` and
    /// `permissionRequest`; every command we install is silent and exits 0,
    /// even when the baked binary is gone, so it neither approves nor denies.
    /// The `bash` form runs through `sh`; on Windows the `powershell` form,
    /// which Copilot runs there, through PowerShell too.
    #[test]
    fn copilot_hook_commands_are_silent_and_never_fail() {
        let gone = Path::new("/nonexistent/mAIestro Code.app/Contents/MacOS/maiestro");
        let file = copilot_hooks(gone, "203-x");
        let events = file["hooks"].as_object().unwrap();
        assert_eq!(events.len(), COPILOT_HOOK_EVENTS.len());
        for (event, handlers) in events {
            let handlers = handlers.as_array().unwrap();
            assert_eq!(handlers.len(), 1, "{event}");
            let bash = handlers[0]["bash"].as_str().unwrap();
            assert!(bash.ends_with(">/dev/null 2>&1 || true"), "{event}: {bash}");
            assert_silent_success(HookShell::Posix, event, bash);
            match HookShell::for_agent(Agent::Copilot) {
                HookShell::Posix => assert!(handlers[0].get("powershell").is_none(), "{event}"),
                HookShell::PowerShell => {
                    let ps = handlers[0]["powershell"].as_str().unwrap();
                    assert!(ps.starts_with("try { & '") && ps.ends_with("exit 0"), "{event}: {ps}");
                    assert_silent_success(HookShell::PowerShell, event, ps);
                }
                HookShell::Cmd => unreachable!("Copilot runs hooks with PowerShell on Windows"),
            }
        }
    }

    /// The file is Copilot's `{"version": 1, "hooks": {…}}` shape with the
    /// documented event → verb map, each command baking in the workspace.
    #[test]
    fn copilot_hooks_map_the_events() {
        let file = copilot_hooks(Path::new("/bin/maiestro"), "203-x");
        assert_eq!(file["version"], 1);
        let verb = |event: &str| {
            let h = &file["hooks"][event][0];
            assert_eq!(h["type"], "command", "{event}");
            assert_eq!(h["timeoutSec"], 10, "{event}");
            h["bash"].as_str().unwrap().to_string()
        };
        assert_eq!(verb("permissionRequest"), "'/bin/maiestro' hook notification --workspace '203-x' >/dev/null 2>&1 || true");
        assert!(verb("sessionStart").contains(" hook session_start "));
        assert!(verb("userPromptSubmitted").contains(" hook prompt "));
        assert!(verb("preToolUse").contains(" hook busy "));
        assert!(verb("postToolUse").contains(" hook tool_ok "));
        assert!(verb("postToolUseFailure").contains(" hook tool_failed "));
        assert!(verb("errorOccurred").contains(" hook error "));
        assert!(verb("agentStop").contains(" hook idle "));
        assert!(verb("sessionEnd").contains(" hook ended "));
    }

    /// Write creates the file, reconcile re-points it at a new binary without
    /// churn and never injects it, and remove deletes it with the folders it
    /// left empty — but not a `.github/` the repo uses.
    #[test]
    fn copilot_hooks_write_reconcile_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        let path = copilot_hook_file(dir.path());
        assert!(!reconcile_copilot_hooks(dir.path(), "203-x", Path::new("/x/maiestro")), "never injects");
        assert!(!path.exists());

        write_copilot_hooks(dir.path(), "203-x", Path::new("/old/maiestro")).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("'/old/maiestro' hook"));
        let new_bin = Path::new("/Applications/mAIestro Code.app/Contents/MacOS/maiestro");
        assert!(reconcile_copilot_hooks(dir.path(), "203-x", new_bin));
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("mAIestro Code.app") && !body.contains("/old/"), "{body}");
        assert!(!reconcile_copilot_hooks(dir.path(), "203-x", new_bin), "no churn when current");

        assert!(remove_copilot_hooks(dir.path()));
        assert!(!dir.path().join(".github").exists(), "empty folders are cleaned up");
        assert!(!remove_copilot_hooks(dir.path()), "nothing left to remove");

        std::fs::create_dir_all(dir.path().join(".github/workflows")).unwrap();
        write_copilot_hooks(dir.path(), "203-x", new_bin).unwrap();
        assert!(remove_copilot_hooks(dir.path()));
        assert!(!dir.path().join(".github/hooks").exists());
        assert!(dir.path().join(".github/workflows").is_dir(), "the repo's own .github stays");
    }

    /// Spawning a Copilot session writes its hook file and adds it to the
    /// repo's `info/exclude`, so it never shows up in `git status`.
    #[tokio::test]
    async fn copilot_hook_file_stays_out_of_git_status() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| std::process::Command::new("git").current_dir(dir.path()).args(args).output().unwrap();
        assert!(git(&["init", "-q"]).status.success());
        assert!(write_session_hooks(dir.path(), "203-x", Agent::Copilot).await.unwrap().is_none(), "no notice");
        assert!(copilot_hook_file(dir.path()).is_file());
        let status = git(&["status", "--porcelain", "--untracked-files=all"]);
        assert_eq!(String::from_utf8_lossy(&status.stdout), "", "nothing to commit");
        let exclude = std::fs::read_to_string(dir.path().join(".git/info/exclude")).unwrap();
        assert!(exclude.lines().any(|l| l == ".github/hooks/maiestro-status.json"), "{exclude}");
    }
}
