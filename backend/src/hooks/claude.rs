//! Claude Code's status hooks, and the quota-reading status line.
//!
//! At worktree creation we write hooks into the worktree's
//! `.claude/settings.local.json` (the personal, gitignored layer that merges with
//! the user's settings and applies to both terminal and VS Code integrated-terminal
//! sessions), merged around any hooks and settings already there. Each hook
//! invokes this binary as `maiestro hook <state> --workspace <ws-id>`; reconcile
//! re-points them at the running binary, and switching a session away from
//! Claude removes them.

use std::path::{Path, PathBuf};

use super::HookShell;
use crate::agent::Agent;

/// The worktree file Claude Code reads mAIestro Code's status hooks from.
pub(super) fn claude_hook_file(work_dir: &Path) -> PathBuf {
    work_dir.join(".claude").join("settings.local.json")
}

/// The Claude half of [`write_session_hooks`](super::write_session_hooks): merge our hooks (pointing at
/// `bin`) into [`claude_hook_file`], keeping every other setting and hook.
pub(super) fn write_claude_hooks(work_dir: &Path, ws_id: &str, bin: &Path) -> Result<(), String> {
    let path = claude_hook_file(work_dir);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }

    // Start from any existing settings, then merge ours in (replacing any prior
    // entries of ours so a changed binary path heals rather than duplicating).
    let root: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .filter(|v: &serde_json::Value| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    let root = merge_hooks(root, bin, ws_id);
    let root = merge_statusline(root, bin, ws_id, || user_statusline_padding(work_dir));

    std::fs::write(&path, serde_json::to_string_pretty(&root).unwrap() + "\n").map_err(|e| e.to_string())
}

/// Build mAIestro Code's Claude status-hook entries (event name → hook group) for
/// a worktree, using `bin` as the helper binary path. Shared by spawn (which writes them) and
/// startup reconcile (which rewrites them at the current binary). The commands run
/// through a POSIX shell (Git Bash on Windows), so the binary path (may contain
/// spaces, e.g. inside "/Applications/.../mAIestro Code.app") and the ws id are
/// single-quoted. Git Bash runs a single-quoted `C:\…\maiestro.exe` as is.
fn maiestro_hook_groups(bin: &Path, ws_id: &str) -> Vec<(&'static str, serde_json::Value)> {
    let shell = HookShell::for_agent(Agent::Claude);
    let cmd = |state: &str| shell.hook_command(bin, state, ws_id);

    // Bare hook group (events that take no matcher).
    let group = |state: &str| {
        serde_json::json!({
            "hooks": [{ "type": "command", "command": cmd(state) }]
        })
    };
    // PreToolUse takes a matcher group; "" matches all tools.
    let matcher_group = |state: &str| {
        serde_json::json!({
            "matcher": "",
            "hooks": [{ "type": "command", "command": cmd(state) }]
        })
    };

    vec![
        ("SessionStart", group("running")),
        // Its own `prompt` verb (not `busy`) so a fresh turn clears a stale
        // failed-tool error while still reading as working.
        ("UserPromptSubmit", group("prompt")),
        ("PreToolUse", matcher_group("busy")),
        // PostToolUse is the event that fires *after* an approved permission
        // prompt's tool completes — the only signal that Claude has resumed
        // working. Without it, a session sticks on `needs_you` (from the prompt's
        // Notification) all the way through the rest of the turn, even while
        // Claude is actively thinking. PreToolUse alone can't cover this: it
        // fires *before* the prompt, not after approval. The distinct `tool_ok`
        // verb (vs PreToolUse's `busy`) also marks a tool *succeeding*, which
        // clears a pending transient `last_error` (issue #48); it still reads as
        // `busy`.
        ("PostToolUse", matcher_group("tool_ok")),
        // A failed tool call: captures the error into `last_error` (kept until
        // dismissed) and logs it. State stays `busy` — Claude works on past it.
        ("PostToolUseFailure", matcher_group("tool_failed")),
        ("Notification", group("notification")),
        ("Stop", group("idle")),
        ("SessionEnd", group("ended")),
    ]
}

/// True when `command` is one of mAIestro Code's status hooks for `ws_id` — matched by
/// the trailing `--workspace '<ws-id>'` we always emit, so unrelated hooks (and
/// other workspaces' hooks) in the same file are left untouched.
fn is_maiestro_hook(command: &str, ws_id: &str) -> bool {
    let ws_q = HookShell::for_agent(Agent::Claude).quote(ws_id);
    command.contains(" hook ") && command.contains(&format!("--workspace {ws_q}"))
}

/// True when a hook *group* contains a command that's one of ours for `ws_id`.
fn group_is_ours(group: &serde_json::Value, ws_id: &str) -> bool {
    group["hooks"]
        .as_array()
        .is_some_and(|hooks| hooks.iter().any(|h| h["command"].as_str().is_some_and(|c| is_maiestro_hook(c, ws_id))))
}

/// True when a parsed settings root already carries any of our hooks for `ws_id`.
fn has_maiestro_hooks(root: &serde_json::Value, ws_id: &str) -> bool {
    root["hooks"].as_object().is_some_and(|events| {
        events
            .values()
            .any(|arr| arr.as_array().is_some_and(|gs| gs.iter().any(|g| group_is_ours(g, ws_id))))
    })
}

/// Merge mAIestro Code's hooks into a parsed settings `root`: for each event, drop any
/// existing entries that are ours (stale paths from a prior spawner), then append
/// a fresh group built from `bin`. Every other hook and setting is preserved.
fn merge_hooks(mut root: serde_json::Value, bin: &Path, ws_id: &str) -> serde_json::Value {
    if !root.is_object() {
        root = serde_json::json!({});
    }
    let mut hooks = serde_json::Value::Object(root["hooks"].as_object().cloned().unwrap_or_default());
    for (event, group) in maiestro_hook_groups(bin, ws_id) {
        let mut arr = hooks[event].as_array().cloned().unwrap_or_default();
        arr.retain(|g| !group_is_ours(g, ws_id));
        arr.push(group);
        hooks[event] = serde_json::Value::Array(arr);
    }
    root["hooks"] = hooks;
    root
}

/// The Claude half of [`reconcile_session_hooks`](super::reconcile_session_hooks) against an explicit `bin`, so
/// tests can prove a stale path is rewritten without depending on the test
/// binary's own path.
pub(super) fn reconcile_claude_hooks(work_dir: &Path, ws_id: &str, bin: &Path) -> bool {
    let path = claude_hook_file(work_dir);
    let Ok(existing) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(root) = serde_json::from_str::<serde_json::Value>(&existing) else {
        return false;
    };
    if !root.is_object() || !has_maiestro_hooks(&root, ws_id) {
        return false;
    }
    let root = merge_hooks(root, bin, ws_id);
    let root = merge_statusline(root, bin, ws_id, || user_statusline_padding(work_dir));
    let updated = serde_json::to_string_pretty(&root).unwrap() + "\n";
    if updated == existing {
        return false;
    }
    std::fs::write(&path, updated).is_ok()
}

// ── Claude: the quota-reading status line ───────────────────────────────────────

/// The status-line command for `ws_id`: this binary's hidden `statusline`
/// subcommand, which records the session's `rate_limits` as the Claude quota
/// and then runs the status line the user would otherwise see (see
/// `crate::quota::run_statusline_cli`).
/// Like the hooks, it runs through a POSIX shell (Git Bash on Windows).
fn statusline_command(bin: &Path, ws_id: &str) -> String {
    let shell = HookShell::for_agent(Agent::Claude);
    format!("{} statusline --workspace {}", shell.program(bin), shell.quote(ws_id))
}

/// True when a `statusLine` setting is ours for `ws_id` — matched like the
/// hooks, by the trailing `--workspace '<ws-id>'`.
fn is_our_statusline(status_line: &serde_json::Value, ws_id: &str) -> bool {
    let ws_q = HookShell::for_agent(Agent::Claude).quote(ws_id);
    status_line["command"]
        .as_str()
        .is_some_and(|c| c.contains(" statusline ") && c.contains(&format!("--workspace {ws_q}")))
}

/// Set our `statusLine` in a parsed `settings.local.json` root, pointing at
/// `bin` — unless the file already has a `statusLine` that isn't ours, which is
/// the user's and is never replaced (that worktree then shows no Claude quota).
/// Re-pointing ours keeps its other fields; a new one copies the `padding` of
/// the status line it chains to (`padding`, called only then).
fn merge_statusline(
    mut root: serde_json::Value,
    bin: &Path,
    ws_id: &str,
    padding: impl FnOnce() -> Option<serde_json::Value>,
) -> serde_json::Value {
    let existing = root.get("statusLine").cloned();
    let mut entry = match existing {
        Some(sl) if is_our_statusline(&sl, ws_id) => sl,
        Some(_) => return root,
        None => {
            let mut sl = serde_json::json!({ "type": "command" });
            if let Some(p) = padding() {
                sl["padding"] = p;
            }
            sl
        }
    };
    entry["command"] = serde_json::Value::String(statusline_command(bin, ws_id));
    root["statusLine"] = entry;
    root
}

/// The `padding` of the user's own status line for this worktree (project
/// settings, then user settings), if they set one.
fn user_statusline_padding(work_dir: &Path) -> Option<serde_json::Value> {
    crate::quota::user_statusline(work_dir).and_then(|sl| sl.get("padding").cloned())
}

/// Remove mAIestro Code's Claude status hooks for `ws_id` from the worktree's
/// [`claude_hook_file`], keeping every other hook and setting — used when a
/// session switches away from Claude (issue #186), so a `claude` run by hand in
/// that worktree no longer writes the session's status. Deletes the file if
/// nothing else is left in it. Best-effort; returns whether it changed anything.
pub fn remove_claude_hooks(work_dir: &Path, ws_id: &str) -> bool {
    let path = claude_hook_file(work_dir);
    let Ok(existing) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(root) = serde_json::from_str::<serde_json::Value>(&existing) else {
        return false;
    };
    if !root.is_object() || !has_maiestro_hooks(&root, ws_id) {
        return false;
    }
    let root = strip_hooks(root, ws_id);
    if root.as_object().is_some_and(|o| o.is_empty()) {
        return std::fs::remove_file(&path).is_ok();
    }
    std::fs::write(&path, serde_json::to_string_pretty(&root).unwrap() + "\n").is_ok()
}

/// Drop our hook groups for `ws_id` from a parsed settings `root`, then any event
/// left empty, then `hooks` itself if empty. Everything else is preserved.
fn strip_hooks(mut root: serde_json::Value, ws_id: &str) -> serde_json::Value {
    let Some(events) = root["hooks"].as_object().cloned() else {
        return root;
    };
    let mut kept = serde_json::Map::new();
    for (event, groups) in events {
        match groups.as_array() {
            Some(gs) => {
                let gs: Vec<_> = gs.iter().filter(|g| !group_is_ours(g, ws_id)).cloned().collect();
                if !gs.is_empty() {
                    kept.insert(event, serde_json::Value::Array(gs));
                }
            }
            None => {
                kept.insert(event, groups);
            }
        }
    }
    let obj = root.as_object_mut().unwrap();
    if obj.get("statusLine").is_some_and(|sl| is_our_statusline(sl, ws_id)) {
        obj.remove("statusLine");
    }
    if kept.is_empty() {
        obj.remove("hooks");
    } else {
        obj.insert("hooks".into(), serde_json::Value::Object(kept));
    }
    root
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Re-merging with a new binary rewrites only *our* hooks for this workspace,
    /// leaving an unrelated user hook and another workspace's hook untouched.
    #[test]
    fn merge_replaces_only_our_hooks_for_this_ws() {
        let ws = "35-status-hooks-fix";
        let old_bin = Path::new("/old/work-8/backend/target/debug/maiestro");

        // A settings file as a prior spawn wrote it, plus an unrelated user hook
        // and a *different* workspace's status hook sharing the Stop event.
        let mut root = merge_hooks(serde_json::json!({}), old_bin, ws);
        let stop = root["hooks"]["Stop"].as_array_mut().unwrap();
        stop.push(serde_json::json!({ "hooks": [{ "type": "command", "command": "echo hi" }] }));
        stop.push(serde_json::json!({
            "hooks": [{ "type": "command", "command": "'/x/maiestro' hook idle --workspace '99-other'" }]
        }));

        let new_bin = Path::new("/Applications/mAIestro Code.app/Contents/MacOS/maiestro");
        let merged = merge_hooks(root, new_bin, ws);

        let cmds: Vec<&str> = merged["hooks"]["Stop"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|g| g["hooks"][0]["command"].as_str())
            .collect();

        // Our stale entry was rewritten to the new binary; the old path is gone.
        assert!(cmds.iter().any(|c| c.contains("/Applications/mAIestro Code.app") && c.contains("--workspace '35-status-hooks-fix'")));
        assert!(!cmds.iter().any(|c| c.contains("/old/work-8")));
        // Exactly one of our entries for this ws remains (no duplication).
        assert_eq!(cmds.iter().filter(|c| is_maiestro_hook(c, ws)).count(), 1);
        // Unrelated and other-workspace hooks survive untouched.
        assert!(cmds.contains(&"echo hi"));
        assert!(cmds.iter().any(|c| c.contains("--workspace '99-other'")));
    }

    /// A worktree carrying our hooks is detected; an unrelated-only file is not.
    #[test]
    fn detects_our_hooks() {
        let ws = "12-foo";
        let ours = merge_hooks(serde_json::json!({}), Path::new("/bin/maiestro"), ws);
        assert!(has_maiestro_hooks(&ours, ws));

        let unrelated = serde_json::json!({
            "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "echo hi" }] }] }
        });
        assert!(!has_maiestro_hooks(&unrelated, ws));
        // Our hooks for a *different* ws don't count as this ws's.
        assert!(!has_maiestro_hooks(&ours, "99-other"));
    }

    /// Reconcile rewrites a Claude worktree's hook file to the current binary.
    #[test]
    fn reconcile_rewrites_the_claude_hook_file() {
        let dir = tempfile::tempdir().unwrap();
        let ws = "9-claude";
        let path = claude_hook_file(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let root = merge_hooks(serde_json::json!({}), Path::new("/old/maiestro"), ws);
        std::fs::write(&path, serde_json::to_string_pretty(&root).unwrap()).unwrap();
        let new_bin = Path::new("/Applications/mAIestro Code.app/Contents/MacOS/maiestro");
        assert!(reconcile_claude_hooks(dir.path(), ws, new_bin));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("/old/maiestro") && text.contains("mAIestro Code.app"), "{text}");
        assert!(!reconcile_claude_hooks(dir.path(), ws, new_bin), "no churn when current");
    }

    /// Our status line is installed with the user's padding, re-pointed (keeping
    /// its fields) on a binary change, and never replaces a foreign one.
    #[test]
    fn merge_statusline_installs_repoints_and_yields() {
        let ws = "205-quota";
        let old = Path::new("/old/maiestro");
        let new = Path::new("/Applications/mAIestro Code.app/Contents/MacOS/maiestro");

        let root = merge_statusline(serde_json::json!({}), old, ws, || Some(serde_json::json!(2)));
        assert_eq!(root["statusLine"]["type"], "command");
        assert_eq!(root["statusLine"]["padding"], 2);
        assert_eq!(root["statusLine"]["command"], "'/old/maiestro' statusline --workspace '205-quota'");

        let root = merge_statusline(root, new, ws, || panic!("padding is only read for a new entry"));
        assert_eq!(root["statusLine"]["padding"], 2);
        assert!(root["statusLine"]["command"].as_str().unwrap().starts_with("'/Applications/mAIestro Code.app"));

        let theirs = serde_json::json!({ "statusLine": { "type": "command", "command": "~/bin/sl" } });
        assert_eq!(merge_statusline(theirs.clone(), new, ws, || None), theirs);
        // Another workspace's status line isn't ours to re-point either.
        let other = merge_statusline(serde_json::json!({}), old, "99-other", || None);
        assert_eq!(merge_statusline(other.clone(), new, ws, || None), other);
    }

    /// Reconcile adds our status line to a worktree that has our hooks but
    /// predates it, and switching away from Claude removes it with the hooks.
    #[test]
    fn reconcile_adds_and_remove_strips_the_status_line() {
        let dir = tempfile::tempdir().unwrap();
        let ws = "205-older";
        let path = claude_hook_file(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bin = Path::new("/x/maiestro");
        let root = merge_hooks(serde_json::json!({}), bin, ws);
        std::fs::write(&path, serde_json::to_string_pretty(&root).unwrap() + "\n").unwrap();

        assert!(reconcile_claude_hooks(dir.path(), ws, bin));
        let root: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(is_our_statusline(&root["statusLine"], ws), "{root}");

        assert!(remove_claude_hooks(dir.path(), ws));
        assert!(!path.exists(), "only ours was in it");

        // A foreign status line survives both.
        let mut root = merge_hooks(serde_json::json!({}), bin, ws);
        root["statusLine"] = serde_json::json!({ "type": "command", "command": "mine" });
        std::fs::write(&path, serde_json::to_string_pretty(&root).unwrap() + "\n").unwrap();
        reconcile_claude_hooks(dir.path(), ws, bin);
        assert!(remove_claude_hooks(dir.path(), ws));
        let left: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(left["statusLine"]["command"], "mine");
    }

    /// Switching away from Claude (#186) strips only our hooks for this
    /// workspace: a user hook, another workspace's hook, and unrelated settings
    /// survive; a file left with nothing in it is deleted.
    #[test]
    fn remove_claude_hooks_keeps_everything_else() {
        let dir = tempfile::tempdir().unwrap();
        let ws = "186-switch";
        let path = claude_hook_file(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bin = Path::new("/x/maiestro");
        let mut root = merge_hooks(serde_json::json!({ "model": "opus" }), bin, ws);
        root["hooks"]["Stop"].as_array_mut().unwrap().push(serde_json::json!({
            "hooks": [{ "type": "command", "command": "echo hi" }]
        }));
        root["hooks"]["Stop"].as_array_mut().unwrap().push(serde_json::json!({
            "hooks": [{ "type": "command", "command": "'/x/maiestro' hook idle --workspace '99-other'" }]
        }));
        std::fs::write(&path, serde_json::to_string_pretty(&root).unwrap()).unwrap();

        assert!(remove_claude_hooks(dir.path(), ws));
        let left: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(!has_maiestro_hooks(&left, ws), "{left}");
        assert_eq!(left["model"], "opus");
        let stop = left["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "{left}");
        assert!(left["hooks"].get("SessionStart").is_none(), "empty events are dropped: {left}");
        assert!(!remove_claude_hooks(dir.path(), ws), "nothing left to remove");

        // Only ours → the file goes away entirely.
        std::fs::write(&path, serde_json::to_string_pretty(&merge_hooks(serde_json::json!({}), bin, ws)).unwrap()).unwrap();
        assert!(remove_claude_hooks(dir.path(), ws));
        assert!(!path.exists());
    }
}
