//! Antigravity's status hooks, a named group in the worktree's `.agents/hooks.json`.
//!
//! Antigravity reads workspace hooks from the worktree's `.agents/hooks.json`, a
//! map of *named* hook groups; we own the `maiestro-status` group
//! ([`antigravity_hook_group`]) and leave any others. Antigravity loads it once
//! the user trusts the folder (its own per-folder prompt at session start, which
//! it shows for any new folder anyway). Unlike Claude, a `PreToolUse` hook's
//! stdout is a *decision* — `{}`, an empty decision, or a failing command all
//! **deny** the tool — so every command we install discards its output and
//! always exits 0 (see [`silent_hook_command`]), and `PreToolUse` is registered
//! only for the tools that ask the user something and the ones that can raise
//! Antigravity's own permission prompt. The file is kept out of git by the
//! worktree's `.gitignore` ([`ensure_antigravity_gitignored`]).

use std::path::{Path, PathBuf};

use super::{hook_wrapper_path, silent_hook_command, write_hook_wrapper, HookShell};
use crate::agent::Agent;

/// The worktree file Antigravity reads workspace hooks from.
pub(super) fn antigravity_hook_file(work_dir: &Path) -> PathBuf {
    work_dir.join(".agents").join("hooks.json")
}

/// The key of mAIestro Code's hook group in [`antigravity_hook_file`]. Hook
/// groups are named, so owning one key is the whole merge.
const ANTIGRAVITY_GROUP: &str = "maiestro-status";

/// The tools through which an Antigravity agent asks the user something — the
/// only explicit "needs you" signal it exposes.
const ANTIGRAVITY_ASK_TOOLS: &str = "ask_question|ask_permission|ask_custom_permission";

/// The tools that can raise Antigravity's own permission prompt ("Run this
/// command?", "Allow creation of this file?", "Accept this file edit?"). That
/// prompt fires no hook, and `PreToolUse` fires *before* it, so the helper
/// records a deadline instead (issue #208): a gated tool still unanswered by
/// then is taken to be waiting on the user. With [`ANTIGRAVITY_ASK_TOOLS`], the
/// only tools our `PreToolUse` hook is registered for.
const ANTIGRAVITY_GATED_TOOLS: &str = "run_command|write_to_file|replace_file_content|multi_replace_file_content";

/// What Antigravity's hook commands run for the running binary `bin`. On
/// Windows it is the stable `.cmd` wrapper ([`hook_wrapper_path`]), re-pointed
/// at `bin` here, rather than `bin` itself: agy runs hooks with `cmd`, where no
/// quotes can be used (see [`HookShell`]), and `cmd` starts a program with its
/// path unquoted, so the program's own argument parsing would split a path with
/// a space and the helper would start as the app. `cmd` parses a batch file's
/// arguments itself, and the wrapper quotes `bin` inside the file. Elsewhere
/// it is `bin`.
pub(super) fn antigravity_hook_program(bin: &Path) -> Result<PathBuf, String> {
    if HookShell::for_agent(Agent::Antigravity) != HookShell::Cmd {
        return Ok(bin.to_path_buf());
    }
    let wrapper = hook_wrapper_path();
    write_hook_wrapper(&wrapper, bin)?;
    Ok(wrapper)
}

/// mAIestro Code's Antigravity hook group for a worktree. Events → helper verbs
/// (see `status::normalize_verb` for the payload-dependent ones):
///
/// - `PreInvocation` → `invocation`: working. It fires before every model call;
///   the first of a turn (`invocationNum` 0) acts as a new prompt.
/// - `PreToolUse` on [`ANTIGRAVITY_ASK_TOOLS`] → `notification`: needs you.
/// - `PreToolUse` on [`ANTIGRAVITY_GATED_TOOLS`] → `gated`: working, with a
///   deadline after which it reads as needs you (see `status::pending_prompt`).
/// - `PostToolUse` (every tool) → `tool_done`: `tool_ok`, or `tool_failed` when
///   the payload carries an `error`.
/// - `Stop` → `stop`: idle (surfacing the stop's `error`, if any).
///
/// There is no session start/end or permission-prompt event to map.
///
/// `program` is what [`antigravity_hook_program`] returned: the binary, or on
/// Windows the wrapper, which supplies the `hook` subcommand itself.
fn antigravity_hook_group(program: &Path, ws_id: &str) -> serde_json::Value {
    let shell = HookShell::for_agent(Agent::Antigravity);
    let handler = |verb: &str| {
        let command = match shell {
            HookShell::Cmd => {
                shell.silent(&format!("{} {verb} --workspace {}", shell.program(program), shell.quote(ws_id)))
            }
            _ => silent_hook_command(shell, program, ws_id, verb),
        };
        serde_json::json!({ "type": "command", "command": command, "timeout": 10 })
    };
    serde_json::json!({
        "PreInvocation": [handler("invocation")],
        "PreToolUse": [
            { "matcher": ANTIGRAVITY_ASK_TOOLS, "hooks": [handler("notification")] },
            { "matcher": ANTIGRAVITY_GATED_TOOLS, "hooks": [handler("gated")] },
        ],
        "PostToolUse": [{ "matcher": "*", "hooks": [handler("tool_done")] }],
        "Stop": [handler("stop")],
    })
}

/// `root` (a parsed `.agents/hooks.json`) with our group set for
/// `program`/`ws_id`. Every other named group is preserved.
fn merge_antigravity_hooks(mut root: serde_json::Value, program: &Path, ws_id: &str) -> serde_json::Value {
    if !root.is_object() {
        root = serde_json::json!({});
    }
    root[ANTIGRAVITY_GROUP] = antigravity_hook_group(program, ws_id);
    root
}

/// Set our group in the worktree's `.agents/hooks.json`, creating it if needed.
/// A file that exists but isn't a JSON object (a repo may commit `.agents/`) is
/// left alone — the session then just shows no status — rather than clobbered.
pub(super) fn write_antigravity_hooks(work_dir: &Path, ws_id: &str, program: &Path) -> Result<(), String> {
    let path = antigravity_hook_file(work_dir);
    let root = match std::fs::read_to_string(&path) {
        Err(_) => serde_json::json!({}),
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(v) if v.is_object() => v,
            _ => {
                tracing::warn!(path = %path.display(), "not installing status hooks: existing .agents/hooks.json isn't a JSON object");
                return Ok(());
            }
        },
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let body = serde_json::to_string_pretty(&merge_antigravity_hooks(root, program, ws_id)).unwrap() + "\n";
    std::fs::write(&path, body).map_err(|e| e.to_string())
}

/// Remove our group from the worktree's `.agents/hooks.json` — used when a
/// session switches away from Antigravity (issue #186), so an `agy` run by hand
/// there no longer writes the session's status. Other groups are kept; a file
/// left empty is deleted. Best-effort; returns whether it changed anything.
pub fn remove_antigravity_hooks(work_dir: &Path) -> bool {
    let path = antigravity_hook_file(work_dir);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(serde_json::Value::Object(mut root)) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    if root.remove(ANTIGRAVITY_GROUP).is_none() {
        return false;
    }
    if root.is_empty() {
        return std::fs::remove_file(&path).is_ok();
    }
    std::fs::write(&path, serde_json::to_string_pretty(&root).unwrap() + "\n").is_ok()
}

/// The Antigravity half of [`reconcile_session_hooks`](super::reconcile_session_hooks) against an explicit
/// `program`: rewrite our group only if the file already has it and it changed.
fn reconcile_antigravity_hooks_with(work_dir: &Path, ws_id: &str, program: &Path) -> bool {
    let path = antigravity_hook_file(work_dir);
    let Ok(existing) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(root) = serde_json::from_str::<serde_json::Value>(&existing) else {
        return false;
    };
    if root.get(ANTIGRAVITY_GROUP).is_none() {
        return false;
    }
    let updated = serde_json::to_string_pretty(&merge_antigravity_hooks(root, program, ws_id)).unwrap() + "\n";
    if updated == existing {
        return false;
    }
    std::fs::write(&path, updated).is_ok()
}

/// The Antigravity half of [`reconcile_session_hooks`](super::reconcile_session_hooks): re-point our group at
/// `program` ([`antigravity_hook_program`]), and put the `.gitignore` line back
/// if someone removed it — but only for a worktree that has our hooks, and
/// without re-warning about a tracked file on every launch.
pub(super) fn reconcile_antigravity_hooks(work_dir: &Path, ws_id: &str, program: &Path) -> bool {
    let rewrote = reconcile_antigravity_hooks_with(work_dir, ws_id, program);
    let notice = antigravity_hook_file(work_dir)
        .is_file()
        .then(|| ensure_antigravity_gitignored(work_dir, false))
        .flatten();
    if let Some(notice) = &notice {
        crate::sessions::set_notice(ws_id, notice.clone());
    }
    rewrote || notice.is_some()
}

// ── Antigravity: keep `.agents/hooks.json` out of git via `.gitignore` ─────────

/// Our Antigravity hook file, relative to the worktree (and as a `.gitignore` line).
const ANTIGRAVITY_HOOK_REL: &str = ".agents/hooks.json";

/// Whether `git check-ignore -v --no-index` output says a **`.gitignore`**
/// ignores the path: its source (the text before the first `:`) is a
/// `.gitignore` file, and the matching pattern isn't a `!` negation. A match in
/// `.git/info/exclude` or the user's global excludes doesn't count — those are
/// local, and the point is that nobody commits the file. Pure for tests.
fn gitignore_covers(check_ignore_v: &str) -> bool {
    let Some(line) = check_ignore_v.lines().find(|l| !l.trim().is_empty()) else {
        return false;
    };
    let Some((meta, _path)) = line.split_once('\t') else {
        return false;
    };
    let mut fields = meta.splitn(3, ':');
    let (Some(source), Some(_line_no), Some(pattern)) = (fields.next(), fields.next(), fields.next()) else {
        return false;
    };
    Path::new(source).file_name().is_some_and(|f| f == ".gitignore") && !pattern.starts_with('!')
}

/// `.gitignore` contents with our block appended, on its own line.
fn gitignore_with_hook_line(existing: &str) -> String {
    let mut body = existing.to_string();
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    if !body.is_empty() {
        body.push('\n');
    }
    body.push_str("# mAIestro Code: local Antigravity status hooks, written per worktree\n");
    body.push_str(ANTIGRAVITY_HOOK_REL);
    body.push('\n');
    body
}

/// Make sure `.agents/hooks.json` — which holds this machine's hook commands and
/// workspace id, never meant to be shared — is ignored by the worktree's
/// `.gitignore`. When no `.gitignore` covers it yet, append it to the
/// worktree-root `.gitignore` (creating it if needed) and return a notice for
/// the user, since that is a change to a tracked file they'll want to commit.
///
/// A `.gitignore` can't hide a file the repo already **tracks**; then return a
/// warning instead when `warn_if_tracked` (spawn and switch — not every
/// startup reconcile, which would re-raise a dismissed warning). Best-effort:
/// git failures are logged and read as "nothing to report".
pub fn ensure_antigravity_gitignored(work_dir: &Path, warn_if_tracked: bool) -> Option<String> {
    let git = |args: &[&str]| crate::tools::command("git").current_dir(work_dir).args(args).output();
    if git(&["ls-files", "--error-unmatch", "--", ANTIGRAVITY_HOOK_REL]).is_ok_and(|o| o.status.success()) {
        tracing::warn!(dir = %work_dir.display(), "the repo tracks .agents/hooks.json; .gitignore can't keep our hooks out of commits");
        return warn_if_tracked.then(|| {
            "This repo tracks `.agents/hooks.json`, so mAIestro Code's Antigravity status hooks in it show up as a \
             change in this worktree. Leave that change out of your commits."
                .to_string()
        });
    }
    match git(&["check-ignore", "-v", "--no-index", "--", ANTIGRAVITY_HOOK_REL]) {
        Ok(o) if gitignore_covers(&String::from_utf8_lossy(&o.stdout)) => return None,
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(error = %e, "couldn't run git check-ignore; not touching .gitignore");
            return None;
        }
    }
    let path = work_dir.join(".gitignore");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if let Err(e) = std::fs::write(&path, gitignore_with_hook_line(&existing)) {
        tracing::warn!(path = %path.display(), error = %e, "couldn't add .agents/hooks.json to .gitignore");
        return Some(format!(
            "Couldn't add `.agents/hooks.json` to this worktree's `.gitignore` ({e}). It holds mAIestro Code's \
             local Antigravity status hooks; keep it out of your commits."
        ));
    }
    tracing::info!(path = %path.display(), "added .agents/hooks.json to .gitignore");
    Some(
        "Added `.agents/hooks.json` to this worktree's `.gitignore`. It holds mAIestro Code's local Antigravity \
         status hooks, which must never be committed. Commit the `.gitignore` change along with your work."
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::assert_silent_success;

    /// Antigravity treats any `PreToolUse` output — even `{}` — or a non-zero
    /// exit as a deny. Every command we install is silent and exits 0, even when
    /// the baked binary is gone (a moved app must never block the user's tools).
    /// Each runs through the shell agy uses on this OS.
    #[test]
    fn antigravity_hook_commands_are_silent_and_never_fail() {
        let shell = HookShell::for_agent(Agent::Antigravity);
        let gone = Path::new("/nonexistent/mAIestro Code.app/Contents/MacOS/maiestro");
        let group = antigravity_hook_group(gone, "185-x");
        let mut commands = Vec::new();
        for (event, handlers) in group.as_object().unwrap() {
            for h in handlers.as_array().unwrap() {
                let hooks = h.get("hooks").and_then(|v| v.as_array()).cloned().unwrap_or_else(|| vec![h.clone()]);
                for hook in hooks {
                    commands.push((event.clone(), hook["command"].as_str().unwrap().to_string()));
                }
            }
        }
        assert_eq!(commands.len(), 5, "{commands:?}");
        for (event, cmd) in &commands {
            match shell {
                HookShell::Posix => assert!(cmd.ends_with(">/dev/null 2>&1 || true"), "{event}: {cmd}"),
                HookShell::PowerShell => {
                    assert!(cmd.starts_with("try { & '") && cmd.ends_with("} catch { }; exit 0"), "{event}: {cmd}")
                }
                HookShell::Cmd => {
                    assert!(cmd.ends_with(" >nul 2>&1 & exit /b 0") && !cmd.contains('"'), "{event}: {cmd}")
                }
            }
            assert_silent_success(shell, event, cmd);
        }
    }

    /// PreToolUse fires only for the tools that ask the user something and the
    /// ones that can raise a permission prompt; the rest of the event map is the
    /// documented one.
    #[test]
    fn antigravity_hook_group_maps_the_events() {
        let group = antigravity_hook_group(Path::new("/bin/maiestro"), "185-x");
        // POSIX runs `<bin> hook <verb>`; on Windows the program is the
        // wrapper, which adds `hook` itself (see `antigravity_hook_program`).
        let shell = HookShell::for_agent(Agent::Antigravity);
        let call = |verb: &str| match shell {
            HookShell::Cmd => format!("/bin/maiestro {verb} --workspace 185-x >nul"),
            _ => format!("'/bin/maiestro' hook {verb} --workspace '185-x' >"),
        };
        let pre = &group["PreToolUse"][0];
        assert_eq!(pre["matcher"], "ask_question|ask_permission|ask_custom_permission");
        assert!(pre["hooks"][0]["command"].as_str().unwrap().starts_with(&call("notification")));
        let gated = &group["PreToolUse"][1];
        assert_eq!(gated["matcher"], "run_command|write_to_file|replace_file_content|multi_replace_file_content");
        assert!(gated["hooks"][0]["command"].as_str().unwrap().starts_with(&call("gated")));
        assert_eq!(group["PreToolUse"].as_array().unwrap().len(), 2);
        assert_eq!(group["PostToolUse"][0]["matcher"], "*");
        assert!(group["PostToolUse"][0]["hooks"][0]["command"].as_str().unwrap().starts_with(&call("tool_done")));
        assert!(group["PreInvocation"][0]["command"].as_str().unwrap().starts_with(&call("invocation")));
        assert!(group["Stop"][0]["command"].as_str().unwrap().starts_with(&call("stop")));
        assert!(group.get("PostInvocation").is_none());
    }

    /// Writing keeps the user's own hook groups, creates the file when absent,
    /// never clobbers a file that isn't a JSON object, and reconcile re-points
    /// our group at a new binary without churn.
    #[test]
    fn antigravity_hooks_merge_and_reconcile() {
        let dir = tempfile::tempdir().unwrap();
        let path = antigravity_hook_file(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"lint": {"PostToolUse": []}}"#).unwrap();
        write_antigravity_hooks(dir.path(), "185-x", Path::new("/old/maiestro")).unwrap();
        let root: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(root.get("lint").is_some() && root.get("maiestro-status").is_some());

        let new_bin = Path::new("/Applications/mAIestro Code.app/Contents/MacOS/maiestro");
        assert!(reconcile_antigravity_hooks_with(dir.path(), "185-x", new_bin));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("/old/maiestro") && text.contains("Code.app") && text.contains("\"lint\""), "{text}");
        assert!(!reconcile_antigravity_hooks_with(dir.path(), "185-x", new_bin), "no churn when current");

        std::fs::write(&path, "not json").unwrap();
        write_antigravity_hooks(dir.path(), "185-x", new_bin).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json");
        assert!(!reconcile_antigravity_hooks_with(dir.path(), "185-x", new_bin));

        let fresh = tempfile::tempdir().unwrap();
        assert!(!reconcile_antigravity_hooks_with(fresh.path(), "185-x", new_bin), "never injects");
        write_antigravity_hooks(fresh.path(), "185-x", new_bin).unwrap();
        assert!(antigravity_hook_file(fresh.path()).is_file());
    }

    /// Switching away from Antigravity drops only our group; a file left empty
    /// is deleted.
    #[test]
    fn remove_antigravity_hooks_keeps_other_groups() {
        let dir = tempfile::tempdir().unwrap();
        let path = antigravity_hook_file(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"lint": {"PostToolUse": []}}"#).unwrap();
        write_antigravity_hooks(dir.path(), "185-x", Path::new("/x/maiestro")).unwrap();
        assert!(remove_antigravity_hooks(dir.path()));
        let left: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(left, serde_json::json!({ "lint": { "PostToolUse": [] } }));
        assert!(!remove_antigravity_hooks(dir.path()), "nothing left to remove");

        std::fs::remove_file(&path).unwrap();
        write_antigravity_hooks(dir.path(), "185-x", Path::new("/x/maiestro")).unwrap();
        assert!(remove_antigravity_hooks(dir.path()));
        assert!(!path.exists());
    }

    /// Only a `.gitignore` match counts — not `info/exclude` or a global
    /// excludes file, and not a `!` negation.
    #[test]
    fn gitignore_covers_reads_check_ignore_output() {
        assert!(gitignore_covers(".gitignore:12:.agents/hooks.json\t.agents/hooks.json\n"));
        assert!(gitignore_covers(".agents/.gitignore:1:hooks.json\t.agents/hooks.json\n"));
        assert!(gitignore_covers(".gitignore:3:.agents/\t.agents/hooks.json\n"));
        assert!(!gitignore_covers("/r/.git/info/exclude:7:.agents/hooks.json\t.agents/hooks.json\n"));
        assert!(!gitignore_covers("/home/u/.config/git/ignore:1:.agents/\t.agents/hooks.json\n"));
        assert!(!gitignore_covers(".gitignore:4:!.agents/hooks.json\t.agents/hooks.json\n"));
        assert!(!gitignore_covers(""));
    }

    /// The line is appended on its own line, after a blank one, whatever the
    /// file's trailing newline; an absent file gets just our block.
    #[test]
    fn gitignore_line_is_appended_cleanly() {
        assert_eq!(gitignore_with_hook_line("node_modules"), "node_modules\n\n# mAIestro Code: local Antigravity status hooks, written per worktree\n.agents/hooks.json\n");
        assert_eq!(gitignore_with_hook_line("a\n"), "a\n\n# mAIestro Code: local Antigravity status hooks, written per worktree\n.agents/hooks.json\n");
        assert_eq!(gitignore_with_hook_line(""), "# mAIestro Code: local Antigravity status hooks, written per worktree\n.agents/hooks.json\n");
    }

    /// Against a real repo: the first call appends the line and returns a
    /// notice, then git really ignores the file and a second call is silent. A
    /// match in `info/exclude` alone doesn't count. A tracked file gets a warning
    /// (only when asked for), and `.gitignore` is left alone.
    #[test]
    fn ensure_antigravity_gitignored_appends_once() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let git = |args: &[&str]| {
            let o = std::process::Command::new("git").current_dir(repo).args(args).output().unwrap();
            assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
        };
        git(&["init", "-q"]);
        std::fs::write(repo.join(".gitignore"), "target/").unwrap();
        std::fs::create_dir_all(repo.join(".git/info")).unwrap();
        std::fs::write(repo.join(".git/info/exclude"), ".agents/hooks.json\n").unwrap();

        let notice = ensure_antigravity_gitignored(repo, true).expect("appended");
        assert!(notice.contains(".gitignore"), "{notice}");
        let gi = std::fs::read_to_string(repo.join(".gitignore")).unwrap();
        assert!(gi.starts_with("target/\n") && gi.ends_with("\n.agents/hooks.json\n"), "{gi:?}");
        assert!(ensure_antigravity_gitignored(repo, true).is_none(), "already covered");
        assert_eq!(std::fs::read_to_string(repo.join(".gitignore")).unwrap(), gi, "no duplicate line");

        // A repo that tracks the file: .gitignore can't help, so warn instead.
        let tracked = tempfile::tempdir().unwrap();
        let t = tracked.path();
        let tgit = |args: &[&str]| {
            let o = std::process::Command::new("git").current_dir(t).args(args).output().unwrap();
            assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
        };
        tgit(&["init", "-q"]);
        std::fs::create_dir_all(t.join(".agents")).unwrap();
        std::fs::write(t.join(".agents/hooks.json"), "{}").unwrap();
        tgit(&["add", ".agents/hooks.json"]);
        assert!(ensure_antigravity_gitignored(t, true).unwrap().contains("tracks"));
        assert!(ensure_antigravity_gitignored(t, false).is_none(), "reconcile doesn't re-warn");
        assert!(!t.join(".gitignore").exists());
    }
}
