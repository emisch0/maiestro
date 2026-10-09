//! Codex's status hooks, passed as session flags on the launch command.
//!
//! Codex trusts a hook only after the user reviews it, recording the approval
//! against the hook's *key and definition hash* in `~/.codex/config.toml`. So
//! Codex hooks must be byte-identical for every worktree, or each spawn would
//! need a fresh review. They are therefore passed on the launch command as
//! `-c hooks.<Event>=…` session flags ([`codex_hook_overrides`]) — whose trust key
//! is `/<session-flags>/config.toml:<event>:…`, with no folder path in it — and
//! run a **stable wrapper** (`~/.maiestro/bin/maiestro-hook <verb>`, see
//! [`ensure_hook_wrapper`](super::ensure_hook_wrapper)) with no workspace id; the
//! helper finds the session from the payload's `cwd`. The user trusts them once
//! (Codex asks at session start → trust all) and never again, across worktrees
//! and mAIestro Code updates (only the wrapper's contents change, not the hook
//! definitions). A project `.codex/hooks.json` is not used: Codex doesn't load it
//! from a spawned worktree at all.

use std::path::Path;

use super::{hook_wrapper_path, HookShell};
use crate::agent::Agent;

/// Codex hook events → helper verbs. The same verbs as Claude's, with two gaps:
/// no `PostToolUseFailure` (so Codex sessions never get a `last_error`) and no
/// `Notification` — `PermissionRequest` is Codex's "needs you" signal, and
/// `status::resolve_state` names the tool when there's no message. The bool marks
/// tool events, which take a matcher group (`""` = every tool).
const CODEX_HOOK_EVENTS: &[(&str, &str, bool)] = &[
    ("SessionStart", "running", false),
    ("UserPromptSubmit", "prompt", false),
    ("PreToolUse", "busy", true),
    ("PostToolUse", "tool_ok", true),
    ("PermissionRequest", "notification", true),
    ("Stop", "idle", false),
    ("SessionEnd", "ended", false),
];

/// Quote `s` as a TOML basic string.
fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The `-c` values that install mAIestro Code's status hooks into a Codex session,
/// one `hooks.<Event>=[…]` TOML override per event, all calling
/// [`hook_wrapper_path`] with just a verb. They are identical for every worktree
/// (the helper resolves the session from the payload's `cwd`), so Codex's
/// per-definition trust is granted once and holds everywhere.
pub fn codex_hook_overrides() -> Vec<String> {
    codex_hook_overrides_for(HookShell::for_agent(Agent::Codex), &hook_wrapper_path())
}

/// The overrides for a wrapper at `wrapper`, run by `shell`: a POSIX shell on
/// macOS, PowerShell on Windows (where Codex runs hooks with `pwsh` or
/// `powershell.exe -NoProfile -Command`).
fn codex_hook_overrides_for(shell: HookShell, wrapper: &Path) -> Vec<String> {
    let wrapper = shell.program(wrapper);
    CODEX_HOOK_EVENTS
        .iter()
        .map(|(event, verb, tool_event)| {
            let handler = format!("{{type=\"command\",command={}}}", toml_string(&format!("{wrapper} {verb}")));
            let matcher = if *tool_event { "matcher=\"\"," } else { "" };
            format!("hooks.{event}=[{{{matcher}hooks=[{handler}]}}]")
        })
        .collect()
}

// ── Codex: will the session ask the user to review our hooks? ──────────────────

/// Whether Codex's `hooks/list` reply shows any of mAIestro Code's session-flag
/// hooks as not yet trusted (`untrusted`, or `modified` after a definition
/// change) — i.e. whether the next Codex session will ask the user to review
/// them. Hooks from other sources (the user's own, a repo's) are ignored. A
/// reply with none of ours is `false`: we only prompt when we know Codex will.
fn hooks_list_needs_review(reply: &serde_json::Value) -> bool {
    reply["result"]["data"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|d| d["hooks"].as_array().into_iter().flatten())
        .filter(|h| h["key"].as_str().is_some_and(|k| k.starts_with("/<session-flags>/")))
        .any(|h| h["trustStatus"].as_str() != Some("trusted"))
}

/// Ask Codex — read-only, through its app-server's `hooks/list` — whether the
/// status hooks a Codex session is launched with ([`codex_hook_overrides`]) still
/// need the user's one-time review. Codex computes trust itself (the stored
/// approval is keyed by hook definition hash), so this is the only reliable way
/// to know. Best-effort: any failure (no `codex`, an older Codex without
/// `hooks/list`, a timeout) reads as `false`, so the popover never nags on a
/// guess. Takes ~0.1s.
pub async fn codex_hooks_need_review() -> bool {
    match tokio::time::timeout(std::time::Duration::from_secs(10), query_codex_hooks()).await {
        Ok(Ok(reply)) => hooks_list_needs_review(&reply),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "couldn't ask codex whether its hooks are trusted");
            false
        }
        Err(_) => {
            tracing::warn!("timed out asking codex whether its hooks are trusted");
            false
        }
    }
}

/// One `initialize` + `hooks/list` round trip with `codex app-server`, with our
/// hooks passed as the same `-c` overrides a session gets.
async fn query_codex_hooks() -> Result<serde_json::Value, String> {
    let cwd = crate::paths::home().to_string_lossy().into_owned();
    codex_app_server_request("hooks/list", serde_json::json!({ "cwds": [cwd] }), &codex_hook_overrides()).await
}

/// One `initialize` + `<method>` round trip with `codex app-server` over stdio
/// JSON-RPC, each of `overrides` passed as a `-c` flag. Returns the whole reply
/// (`{ id, result }`); a JSON-RPC `error` is an `Err`. Shared by the hook-trust
/// check and the quota read (`crate::quota`). No timeout of its own — callers
/// bound it.
pub(crate) async fn codex_app_server_request(
    method: &str,
    params: serde_json::Value,
    overrides: &[String],
) -> Result<serde_json::Value, String> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let mut cmd = crate::tools::tokio_command("codex");
    cmd.arg("app-server");
    for ov in overrides {
        cmd.arg("-c").arg(ov);
    }
    let mut child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("could not run codex: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let mut lines = BufReader::new(child.stdout.take().ok_or("no stdout")?).lines();
    for msg in [
        serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "clientInfo": { "name": "maiestro", "version": env!("CARGO_PKG_VERSION") } } }),
        serde_json::json!({ "jsonrpc": "2.0", "method": "initialized" }),
        serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": method, "params": params }),
    ] {
        stdin.write_all(format!("{msg}\n").as_bytes()).await.map_err(|e| e.to_string())?;
    }
    stdin.flush().await.map_err(|e| e.to_string())?;
    while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if v["id"] == 2 {
            if let Some(err) = v.get("error") {
                return Err(format!("{method} failed: {err}"));
            }
            return Ok(v);
        }
    }
    Err("codex app-server closed before answering".into())
}

/// Whether opening this session (or, with no session, spawning in this repo)
/// will start a Codex session that asks the user to review mAIestro Code's status
/// hooks — so the popover can explain the one-time "trust all" step
/// *before* VS Code opens. The agent is the session record's when given (a reopen
/// launches what the worktree was spawned with), else the repo's effective agent.
/// Always `false` for Claude.
#[tauri::command]
pub async fn codex_hooks_review_needed(repo: String, session_id: Option<String>) -> Result<bool, String> {
    crate::log_invoke_debug!("codex_hooks_review_needed", repo = %repo);
    let agent = match session_id.as_deref().and_then(crate::sessions::get) {
        Some(s) => s.agent,
        None => crate::repo_settings::effective_agent(&crate::repo_settings::repo_settings_get(repo)?),
    };
    if agent != Agent::Codex {
        return Ok(false);
    }
    Ok(codex_hooks_need_review().await)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On Windows the Codex hooks call the `.cmd` wrapper through PowerShell's
    /// call operator, still with no workspace id.
    #[test]
    fn codex_hook_overrides_on_windows_use_powershell() {
        let ov = codex_hook_overrides_for(HookShell::PowerShell, Path::new(r"C:\Users\u\.maiestro\bin\maiestro-hook.cmd"));
        assert!(
            ov.contains(&r#"hooks.Stop=[{hooks=[{type="command",command="& 'C:\\Users\\u\\.maiestro\\bin\\maiestro-hook.cmd' idle"}]}]"#.to_string()),
            "{ov:?}"
        );
    }

    /// The Codex hooks are the same for every worktree — no workspace id, no
    /// worktree path — so one Codex trust approval covers them all. Each event
    /// calls the wrapper with its verb; tool events carry a match-all matcher.
    #[test]
    fn codex_hook_overrides_are_worktree_independent() {
        let ov = codex_hook_overrides_for(HookShell::Posix, Path::new("/home/u/.maiestro/bin/maiestro-hook"));
        assert_eq!(ov.len(), 7);
        assert!(ov.contains(&r#"hooks.Stop=[{hooks=[{type="command",command="'/home/u/.maiestro/bin/maiestro-hook' idle"}]}]"#.to_string()), "{ov:?}");
        assert!(ov.contains(&r#"hooks.PermissionRequest=[{matcher="",hooks=[{type="command",command="'/home/u/.maiestro/bin/maiestro-hook' notification"}]}]"#.to_string()), "{ov:?}");
        assert!(!ov.iter().any(|o| o.contains("PostToolUseFailure") || o.contains("Notification=") || o.contains("--workspace")));
    }

    /// Only our session-flag hooks count, and any state but `trusted` needs review.
    #[test]
    fn needs_review_reads_our_hooks_trust_status() {
        let reply = |hooks: serde_json::Value| serde_json::json!({ "id": 2, "result": { "data": [{ "cwd": "/h", "hooks": hooks }] } });
        let ours = |status: &str| serde_json::json!({ "key": "/<session-flags>/config.toml:stop:0:0", "trustStatus": status });
        assert!(!hooks_list_needs_review(&reply(serde_json::json!([ours("trusted"), ours("trusted")]))));
        assert!(hooks_list_needs_review(&reply(serde_json::json!([ours("trusted"), ours("untrusted")]))));
        assert!(hooks_list_needs_review(&reply(serde_json::json!([ours("modified")]))));
        // Someone else's untrusted hook isn't ours to explain; no hooks at all → don't nag.
        let theirs = serde_json::json!({ "key": "/h/.codex/config.toml:stop:0:0", "trustStatus": "untrusted" });
        assert!(!hooks_list_needs_review(&reply(serde_json::json!([theirs]))));
        assert!(!hooks_list_needs_review(&reply(serde_json::json!([]))));
        assert!(!hooks_list_needs_review(&serde_json::json!({ "id": 2, "error": {} })));
    }

    /// A wrapper path with a quote or backslash in it still yields valid TOML.
    #[test]
    fn toml_string_escapes() {
        assert_eq!(toml_string(r#"a"b\c"#), r#""a\"b\\c""#);
    }
}
