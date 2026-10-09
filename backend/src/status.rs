//! Per-session live status: **busy / needs-you / idle**.
//!
//! mAIestro Code launches the repo's agent (`claude`, `codex`, `agy` or `copilot`) into VS Code and
//! no longer owns its stdio (see CLAUDE.md → "mAIestro Code launches sessions; it
//! does not host them"), so it can't read working/waiting state from the stream.
//! Instead, each spawned worktree gets agent hooks (written by `hooks.rs`) that
//! invoke this very binary as `maiestro hook <state> --workspace <ws-id>`. The
//! hook reads the agent's event JSON on stdin, writes a small status record to `~/.maiestro/status/<ws-id>.json`,
//! and the backend watches that directory and pushes changes to the popover.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A single session's current status, written by the `hook` helper and watched
/// by the backend. Keyed on disk by `<workspace>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusRecord {
    /// Workspace id (= `Session.id`, e.g. "8-surface-per-session").
    pub workspace: String,
    /// One of: creating | running | busy | needs_you | idle | ended. `creating`
    /// is written by `spawn.rs` while a fresh worktree is being built in the
    /// background (not a Claude hook state) and is cleared once the worktree is
    /// ready, after which Claude's own hooks own the record.
    pub state: String,
    /// Claude session id from the hook payload, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The worktree cwd from the hook payload, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Short human detail for the UI (e.g. "permission: Bash", a tool name, the
    /// notification message).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The most recent failed tool call, if any. Unlike `state`/`detail` (which
    /// the next hook overwrites within seconds), this is preserved across writes
    /// until the user dismisses it or starts a new turn — so a failure stays
    /// visible long enough to be read. See `run_hook_cli`'s lifecycle rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<ToolError>,
    /// An Antigravity tool that may be waiting on agy's own permission prompt,
    /// which fires no hook (issue #208). Set by a `gated` hook; if no later hook
    /// replaces the record by `deadline`, the backend promotes it to `needs_you`
    /// ([`promote_overdue`]). Never carried forward, so any later hook clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_after: Option<PendingPrompt>,
    /// RFC-3339 timestamp of the transition.
    pub ts: String,
}

/// A possible pending permission prompt — see [`StatusRecord::prompt_after`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingPrompt {
    /// RFC-3339 time after which the session reads as waiting on the user.
    pub deadline: String,
    /// The `needs_you` detail to show then (e.g. the command awaiting approval).
    pub detail: String,
}

/// One failed tool call. Logged on every failure, but only shown in the popover
/// once `surfaced` is true — see the `last_error` lifecycle in `run_hook_cli`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolError {
    /// The tool that failed (from the hook payload's `tool_name`), when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// The error message extracted from the `PostToolUseFailure` payload.
    pub message: String,
    /// RFC-3339 timestamp of the (most recent) failure.
    pub ts: String,
    /// Consecutive failures of this same tool. A second strike (>=
    /// `RETRY_SURFACE_THRESHOLD`) marks the error persistent and surfaces it.
    #[serde(default)]
    pub count: u32,
    /// Whether this error has been promoted to prominent popover display. While
    /// false the failure is logged and tracked but hidden — Claude may still
    /// recover. Promoted on Stop/idle (Claude stopped without recovering) or a
    /// repeated same-tool failure; cleared when a later tool succeeds.
    #[serde(default)]
    pub surfaced: bool,
}

/// Number of consecutive same-tool failures at which a still-pending error is
/// promoted to prominent display (the second strike). Not user-configurable.
const RETRY_SURFACE_THRESHOLD: u32 = 2;

/// How long past `WaitMsBeforeAsync` an Antigravity `run_command` may go without
/// another hook before it reads as waiting on its permission prompt. Without a
/// prompt, the next hook lands by then: `PostToolUse` if the command finished,
/// or the next `PreInvocation` once agy moves it to the background.
const COMMAND_PROMPT_GRACE_MS: u64 = 2_000;

/// `WaitMsBeforeAsync` when a `run_command` payload lacks it, and the cap on it,
/// so a huge value can't hold back "needs you" for minutes.
const DEFAULT_COMMAND_WAIT_MS: u64 = 5_000;
const MAX_COMMAND_WAIT_MS: u64 = 60_000;

/// How long an Antigravity file write/edit may go without another hook before
/// it reads as waiting on its permission prompt. Unprompted, these finish (and
/// fire `PostToolUse`) at once.
const EDIT_PROMPT_GRACE_MS: u64 = 3_000;

fn status_dir() -> PathBuf {
    crate::paths::maiestro_dir("status")
}

fn status_path(ws: &str) -> PathBuf {
    status_dir().join(format!("{ws}.json"))
}

/// True if `ws` is safe to use as a single status-file component — non-empty, not
/// `.`/`..`, and containing no path separator or NUL. Internally generated ids
/// (`<issue>-<slug>`, slug is `[a-z0-9-]`) always pass; this only rejects crafted
/// input reaching the untrusted boundaries (`run_hook_cli`'s `--workspace` argv and
/// the `clear_session_error` command), which could otherwise make `status_path`
/// escape `~/.maiestro/status/` via `..`/`/` and write attacker-controlled JSON to
/// an arbitrary user-writable path.
fn is_safe_workspace_id(ws: &str) -> bool {
    !ws.is_empty()
        && ws != "."
        && ws != ".."
        && !ws.contains('/')
        && !ws.contains('\\')
        && !ws.contains('\0')
}

// ── Hook CLI (`maiestro hook <state> --workspace <ws-id>`) ──────────────────────

/// The tool a hook payload is about: Claude and Codex send `tool_name`,
/// Antigravity a camelCase `toolCall.name`, Copilot a camelCase `toolName`.
fn tool_name(payload: &serde_json::Value) -> Option<String> {
    payload["tool_name"]
        .as_str()
        .or_else(|| payload["toolCall"]["name"].as_str())
        .or_else(|| payload["toolName"].as_str())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

/// A non-empty `error` from a payload: a string on Antigravity's
/// `PostToolUse`/`Stop` (both always carry the field, as `""` when nothing went
/// wrong) and Copilot's `postToolUseFailure`, or an object's `message` (as
/// Copilot's `errorOccurred` documents it).
fn payload_error(payload: &serde_json::Value) -> Option<&str> {
    payload["error"]
        .as_str()
        .or_else(|| payload["error"]["message"].as_str())
        .map(str::trim)
        .filter(|e| !e.is_empty())
}

/// The agent's own session id: Claude and Codex send `session_id`, Antigravity
/// `conversationId`, Copilot `sessionId`.
fn payload_session_id(payload: &serde_json::Value) -> Option<&str> {
    payload["session_id"]
        .as_str()
        .or_else(|| payload["conversationId"].as_str())
        .or_else(|| payload["sessionId"].as_str())
}

/// Whether a Copilot `sessionStart` arrived after its session's turn already
/// began (Copilot can fire it *after* `userPromptSubmitted`), so recording it
/// would overwrite a working state. Only the same Copilot session counts: a new
/// session after one that died mid-turn still resets the record.
fn session_start_is_late(prior: Option<&StatusRecord>, payload: &serde_json::Value) -> bool {
    prior.is_some_and(|r| {
        matches!(r.state.as_str(), "busy" | "needs_you")
            && r.session_id.is_some()
            && r.session_id.as_deref() == payload_session_id(payload)
    })
}

/// Turn the verbs whose meaning depends on the payload into the ones below.
/// Antigravity's events are coarser than Claude's, so its hooks pass these and
/// the helper decides here:
///
/// - `invocation` (`PreInvocation`, before *every* model call): the first call
///   of a turn (`invocationNum` 0) is a fresh `prompt` — clearing a stale error —
///   and later ones are just `busy`.
/// - `tool_done` (`PostToolUse`): `tool_failed` when it carries an `error`,
///   else `tool_ok`.
/// - `stop` (`Stop`): `idle` (its `error` is surfaced by `run_hook_cli`).
/// - `gated` (`PreToolUse` on a tool that may prompt) passes through: `busy`,
///   plus a [`pending_prompt`] deadline set by `run_hook_cli`.
///
/// Copilot's `sessionStart` passes `session_start`, which is `running` (when it
/// isn't late — see [`session_start_is_late`]). Every other verb passes through
/// unchanged.
fn normalize_verb<'a>(arg: &'a str, payload: &serde_json::Value) -> &'a str {
    match arg {
        "invocation" if payload["invocationNum"].as_u64().unwrap_or(0) == 0 => "prompt",
        "invocation" => "busy",
        "tool_done" if payload_error(payload).is_some() => "tool_failed",
        "tool_done" => "tool_ok",
        "stop" => "idle",
        "session_start" => "running",
        other => other,
    }
}

/// Map the CLI state argument + the parsed hook payload into the record's
/// `state` and `detail`. `notification` is the only one that inspects the
/// payload to choose between waiting-on-permission and an idle nudge — both
/// surface as `needs_you`, differing only in `detail`.
fn resolve_state(arg: &str, payload: &serde_json::Value) -> (String, Option<String>) {
    match arg {
        "running" => ("running".into(), None),
        // UserPromptSubmit: a fresh turn. Same `busy` state as a running tool,
        // but its own verb so the helper can clear a stale `last_error`.
        "prompt" => ("busy".into(), None),
        // PreToolUse carries the tool name; UserPromptSubmit does not.
        "busy" => ("busy".into(), tool_name(payload)),
        // Antigravity's PreToolUse on a tool that may raise its own permission
        // prompt: working for now; `run_hook_cli` adds the `prompt_after` deadline.
        "gated" => ("busy".into(), tool_name(payload)),
        // A failed tool call. Claude keeps working after it, so the *state* stays
        // `busy`; the failure itself rides on `last_error` (set in run_hook_cli).
        "tool_failed" => ("busy".into(), tool_name(payload)),
        // PostToolUse: a tool *completed successfully*. Reads as `busy` like any
        // other working signal; its own verb (vs `busy`/PreToolUse) lets the
        // helper clear a pending `last_error` — Claude recovered and moved on.
        "tool_ok" => ("busy".into(), tool_name(payload)),
        // Claude's `Notification` carries a `message`. Codex has no such event
        // and fires this verb from `PermissionRequest`, whose payload names the
        // tool instead — so fall back to that for the detail. Antigravity fires
        // it from `PreToolUse` on its asking tools; an `ask_question` call shows
        // its (first) question.
        "notification" => {
            let msg = payload["message"].as_str().unwrap_or("").trim();
            let question = payload["toolCall"]["args"]["questions"][0]["question"].as_str().unwrap_or("").trim();
            let detail = if !msg.is_empty() {
                Some(msg.to_string())
            } else if !question.is_empty() {
                Some(question.to_string())
            } else {
                tool_name(payload).map(|t| format!("Permission requested: `{t}`"))
            };
            ("needs_you".into(), detail)
        }
        "idle" => ("idle".into(), None),
        "ended" => ("ended".into(), None),
        // Copilot's `errorOccurred`: still inside the turn (an `agentStop` or
        // `sessionEnd` follows if it gave up); the error rides on `last_error`.
        "error" => ("busy".into(), None),
        // Unknown verb: record it verbatim rather than guessing.
        other => (other.into(), None),
    }
}

/// Decide a workspace's next `last_error` from the incoming event and the carried
/// prior value. Pure (no I/O) so the surfacing rules can be unit-tested.
///
/// The status file is last-write-wins and the next hook lands within seconds, so
/// a failure can't live in `state`. The error is logged on every failure but only
/// *surfaced* (shown in the popover) once it proves to matter — Claude often
/// recovers and works straight past a transient failure (issue #48):
///
/// - `tool_failed` → set/refresh it (pending). A repeated same-tool failure
///   (`count >= RETRY_SURFACE_THRESHOLD`) is persistent → surface.
/// - `tool_ok` → a tool succeeded; clear a still-*pending* error (Claude
///   recovered). A surfaced one stays (user dismisses it).
/// - `idle` → Claude stopped without recovering → surface a pending error.
/// - `prompt`/`running` → new turn/session clears it.
/// - everything else → carry the prior value forward (survives until dismissed).
///
/// `failure` carries the freshly-parsed `(tool, message)` on a `tool_failed`
/// event and is `None` otherwise.
fn next_last_error(
    state_arg: &str,
    prior: Option<ToolError>,
    failure: Option<(Option<String>, String)>,
    ts: &str,
) -> Option<ToolError> {
    match state_arg {
        "tool_failed" => {
            let (tool, message) = failure?;
            // A consecutive failure of the *same* tool bumps the count; a different
            // tool (or first failure) resets to 1.
            let count = prior.filter(|e| e.tool == tool).map_or(1, |e| e.count + 1);
            Some(ToolError {
                tool,
                message,
                ts: ts.to_string(),
                count,
                surfaced: count >= RETRY_SURFACE_THRESHOLD,
            })
        }
        "tool_ok" => prior.filter(|e| e.surfaced),
        "idle" => prior.map(|mut e| {
            e.surfaced = true;
            e
        }),
        "prompt" | "running" => None,
        _ => prior,
    }
}

/// Entry point for `maiestro hook …`, dispatched from `main()` before Tauri
/// starts. Reads the hook payload from stdin, writes the status record, and
/// returns. Deliberately failure-tolerant: a hook must never block or crash the
/// user's Claude session, so every error is swallowed.
pub fn run_hook_cli(args: &[String]) {
    // args = ["<state>", "--workspace", "<ws-id>"] (order-tolerant for the flag).
    // Codex hooks pass only the state: they must be identical for every worktree
    // (see hooks.rs), so the workspace comes from the payload's `cwd` instead.
    let raw_arg = args.first().map(|s| s.as_str()).unwrap_or("");
    let flag = args.iter().position(|a| a == "--workspace").and_then(|i| args.get(i + 1)).cloned();
    if raw_arg.is_empty() {
        return;
    }

    // Best-effort stdin parse: the agent sends a JSON event, but we still record
    // a status even if it's empty or malformed.
    let mut buf = String::new();
    let _ = std::io::stdin().read_to_string(&mut buf);
    let payload: serde_json::Value = serde_json::from_str(&buf).unwrap_or(serde_json::Value::Null);
    let state_arg = normalize_verb(raw_arg, &payload);

    let workspace = match flag {
        Some(ws) => ws,
        None => {
            let Some(cwd) = payload["cwd"].as_str() else { return };
            let sessions: Vec<(String, String)> =
                crate::sessions::load_all().into_iter().map(|s| (s.id, s.work_dir)).collect();
            let Some(ws) = workspace_for_cwd(cwd, &sessions) else { return };
            ws
        }
    };
    if !is_safe_workspace_id(&workspace) {
        return;
    }

    let prior_record = read_record(&workspace);
    if raw_arg == "session_start" && session_start_is_late(prior_record.as_ref(), &payload) {
        return;
    }

    let (state, detail) = resolve_state(state_arg, &payload);
    let now = chrono::Utc::now();
    let ts = now.to_rfc3339();
    let prompt_after = (state_arg == "gated").then(|| pending_prompt(&payload, now));

    // `last_error` lifecycle (see `next_last_error`). On a failure, hand the raw
    // tool+message to the decision fn (it computes the count and surfacing) and
    // then log the result — surfacing is gated, logging is not.
    let prior = prior_record.and_then(|r| r.last_error);
    let failure = (state_arg == "tool_failed").then(|| (tool_name(&payload), extract_error_message(&payload)));
    let mut last_error = next_last_error(state_arg, prior, failure, &ts);
    // An Antigravity turn that stopped *because of* an error: surface it.
    if raw_arg == "stop" {
        if let Some(error) = payload_error(&payload) {
            last_error = Some(stop_error(error, &ts));
            crate::logging::append_line(&format!("session={workspace} agent stopped with an error: {error}"));
        }
    }
    // A Copilot `errorOccurred`: surface it, whatever it carried.
    if raw_arg == "error" {
        let error = payload_error(&payload).unwrap_or("The agent reported an error");
        last_error = Some(stop_error(error, &ts));
        crate::logging::append_line(&format!("session={workspace} agent reported an error: {error}"));
    }
    if state_arg == "tool_failed" {
        if let Some(err) = &last_error {
            crate::logging::append_line(&format!(
                "session={workspace} tool call failed [{}] (#{}): {}",
                err.tool.as_deref().unwrap_or("?"),
                err.count,
                err.message
            ));
        }
    }

    let record = StatusRecord {
        workspace: workspace.clone(),
        state,
        // Antigravity: `workspacePaths` instead of `cwd`.
        session_id: payload_session_id(&payload).map(str::to_string),
        cwd: payload["cwd"].as_str().or_else(|| payload["workspacePaths"][0].as_str()).map(str::to_string),
        detail,
        last_error,
        prompt_after,
        ts,
    };

    let _ = write_record_atomic(&record);
}

/// The deadline and detail for an Antigravity `gated` hook (issue #208). agy's
/// own permission prompt fires no hook, and its `PreToolUse` fires *before* the
/// prompt, with nothing in the payload to say one is coming. So we wait: a
/// `run_command` gets its `WaitMsBeforeAsync` (after which agy backgrounds an
/// unprompted command and calls the model again) plus a grace; a file write or
/// edit, which is instant when unprompted, gets a short grace.
fn pending_prompt(payload: &serde_json::Value, now: chrono::DateTime<chrono::Utc>) -> PendingPrompt {
    let tool = tool_name(payload).unwrap_or_else(|| "tool".into());
    let args = &payload["toolCall"]["args"];
    let command = args["CommandLine"].as_str().map(str::trim).filter(|c| !c.is_empty());
    let (wait_ms, detail) = if tool == "run_command" {
        let wait = args["WaitMsBeforeAsync"].as_u64().unwrap_or(DEFAULT_COMMAND_WAIT_MS).min(MAX_COMMAND_WAIT_MS);
        let detail = match command {
            Some(cmd) => format!("Run this command? `{}`", shorten(&redact_secrets(cmd), 120)),
            None => "Run this command?".to_string(),
        };
        (wait + COMMAND_PROMPT_GRACE_MS, detail)
    } else {
        (EDIT_PROMPT_GRACE_MS, format!("Permission requested: `{tool}`"))
    };
    let deadline = now + chrono::Duration::milliseconds(wait_ms as i64);
    PendingPrompt { deadline: deadline.to_rfc3339(), detail }
}

/// `s` cut to at most `max` characters, with an ellipsis when cut.
fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// `record` promoted to `needs_you` if its [`PendingPrompt`] deadline has passed
/// by `now`, else `None`. A deadline that doesn't parse is treated as passed, so
/// a damaged record can't stay pending forever. Pure, so the rule is testable.
fn promote_overdue(record: &StatusRecord, now: chrono::DateTime<chrono::Utc>) -> Option<StatusRecord> {
    let pending = record.prompt_after.as_ref()?;
    let due = chrono::DateTime::parse_from_rfc3339(&pending.deadline).map_or(true, |d| now >= d);
    due.then(|| StatusRecord {
        state: "needs_you".into(),
        detail: Some(pending.detail.clone()),
        prompt_after: None,
        ts: now.to_rfc3339(),
        ..record.clone()
    })
}

/// Promote `ws`'s record to `needs_you` if it is still the one written at `ts`
/// (no later hook replaced it) and its deadline has passed. The watcher calls
/// this when a `prompt_after` deadline comes due; the rewrite re-emits through
/// the watcher. The read-then-write can, in a very narrow window, overwrite a
/// hook landing at the same instant, which the next hook then corrects.
fn promote_if_unchanged(ws: &str, ts: &str) {
    let Some(record) = read_record(ws).filter(|r| r.ts == ts) else { return };
    if let Some(promoted) = promote_overdue(&record, chrono::Utc::now()) {
        let _ = write_record_atomic(&promoted);
    }
}

/// The `last_error` for an Antigravity `Stop` that carried an `error`, or a
/// Copilot `errorOccurred`: surfaced at once, since the agent has already
/// stopped or hit something it couldn't work past.
fn stop_error(error: &str, ts: &str) -> ToolError {
    ToolError { tool: None, message: redact_secrets(error), ts: ts.to_string(), count: 1, surfaced: true }
}

/// The workspace whose worktree contains `cwd` — the session's own directory or
/// any folder inside it — from `(id, work_dir)` pairs. The deepest match wins, so
/// a worktree nested under another (unusual, but possible with a custom prefix)
/// resolves to itself. `None` when `cwd` is in no tracked worktree (a Codex
/// session mAIestro Code didn't launch), and the hook then records nothing.
/// Paths are compared through [`comparable_path`], so a Windows `cwd` matches
/// whichever separators and drive-letter case either side uses.
fn workspace_for_cwd(cwd: &str, sessions: &[(String, String)]) -> Option<String> {
    let cwd = comparable_path(cwd);
    sessions
        .iter()
        .filter(|(_, dir)| {
            let dir = comparable_path(dir);
            !dir.is_empty() && (cwd == dir || cwd.strip_prefix(&dir).is_some_and(|rest| rest.starts_with('/')))
        })
        .max_by_key(|(_, dir)| dir.len())
        .map(|(id, _)| id.clone())
}

/// `path` with `/` separators and no trailing one; on Windows also lowercased,
/// since its paths are case-insensitive and an agent may report `c:\…` for a
/// worktree recorded as `C:\…`.
fn comparable_path(path: &str) -> String {
    let path = if cfg!(windows) { path.replace('\\', "/").to_lowercase() } else { path.to_string() };
    path.trim_end_matches('/').to_string()
}

/// Pull a human-readable failure message out of a `PostToolUseFailure` payload.
/// The exact field isn't pinned down in the docs, so try the likely ones in
/// order and fall back to a generic message rather than dropping the failure.
fn extract_error_message(payload: &serde_json::Value) -> String {
    let candidates = [
        payload_error(payload),
        payload["tool_response"]["error"].as_str(),
        payload["tool_response"]["stderr"].as_str(),
        payload["message"].as_str(),
    ];
    for c in candidates.into_iter().flatten() {
        let trimmed = c.trim();
        if !trimmed.is_empty() {
            return redact_secrets(trimmed);
        }
    }
    "Tool call failed".to_string()
}

/// Redact credentials that can appear in a tool's error text before it is written
/// to the persistent log or shown in the popover. A failed session command (e.g.
/// `curl https://user:token@host/…`) can put a secret in stderr; this masks the
/// `user:pass@` userinfo of any URL in the message, and bounds the length so a huge
/// error can't bloat the log. Dependency-free (no regex) to keep the hook helper
/// minimal, matching the rest of the hook path.
fn redact_secrets(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i..].starts_with("://") {
            out.push_str("://");
            i += 3;
            // The authority runs until the next path/query/fragment/quote/space.
            let start = i;
            while i < s.len()
                && !matches!(bytes[i], b'/' | b'?' | b'#' | b' ' | b'\t' | b'"' | b'\'')
            {
                i += 1;
            }
            let authority = &s[start..i];
            match authority.rfind('@') {
                Some(at) => {
                    out.push_str("***@");
                    out.push_str(&authority[at + 1..]);
                }
                None => out.push_str(authority),
            }
        } else {
            let ch = s[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out.chars().take(500).collect()
}

/// Read a workspace's current status record from disk, if present and valid.
fn read_record(ws: &str) -> Option<StatusRecord> {
    let data = std::fs::read_to_string(status_path(ws)).ok()?;
    serde_json::from_str(&data).ok()
}

/// Write `~/.maiestro/status/<ws>.json` atomically (temp file + rename) so the
/// watcher never reads a half-written record.
fn write_record_atomic(record: &StatusRecord) -> std::io::Result<()> {
    // Defense in depth: never let a crafted workspace id escape the status dir via
    // the temp/final path, even if a future caller skips the entry-point checks.
    if !is_safe_workspace_id(&record.workspace) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "unsafe workspace id",
        ));
    }
    let dir = status_dir();
    std::fs::create_dir_all(&dir)?;
    let data = serde_json::to_string_pretty(record).unwrap_or_default();
    let final_path = status_path(&record.workspace);
    let tmp = dir.join(format!(".{}.tmp", record.workspace));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, &final_path)
}

// ── Backend read side: list, sweep, watch ──────────────────────────────────────

/// All current status records on disk.
fn load_all() -> Vec<StatusRecord> {
    let Ok(entries) = std::fs::read_dir(status_dir()) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|e| {
            let data = std::fs::read_to_string(e.path()).ok()?;
            serde_json::from_str(&data).ok()
        })
        .collect()
}

/// Remove a workspace's status file (called on teardown). The watcher then
/// emits an `ended` record so the popover clears the row. No-op if absent.
pub fn remove(ws: &str) {
    let _ = std::fs::remove_file(status_path(ws));
}

/// Mark a workspace as `creating` — the worktree is being built in the
/// background (`spawn.rs`). Written synchronously at spawn start, *before* the
/// `Session` row's heavy work, so the popover shows a "Creating…" pill right
/// away. Not a Claude hook state; cleared by `clear_creating` when the worktree
/// is ready or replaced by `write_spawn_error` on failure.
pub fn write_creating(ws: &str) {
    let _ = write_record_atomic(&StatusRecord {
        workspace: ws.to_string(),
        state: "creating".into(),
        session_id: None,
        cwd: None,
        detail: None,
        last_error: None,
        prompt_after: None,
        ts: chrono::Utc::now().to_rfc3339(),
    });
}

/// Clear a `creating` marker once the worktree is ready, so Claude's own hooks
/// (SessionStart → running, …) take over the record. Removes the file only if it
/// still reads `creating`, so a status Claude may already have written (it can't,
/// since this runs before the editor opens — belt and braces) is never clobbered.
/// The watcher then emits `ended`, clearing the pill until Claude's first hook.
pub fn clear_creating(ws: &str) {
    if read_record(ws).map(|r| r.state == "creating").unwrap_or(false) {
        remove(ws);
    }
}

/// Record a failed background spawn as a surfaced `last_error` on the workspace's
/// status, reusing the dismissible error block the popover already shows for
/// failed tool calls. The `Session` row is kept so the user can read the error
/// and tear the broken workspace down.
pub fn write_spawn_error(ws: &str, message: &str) {
    let ts = chrono::Utc::now().to_rfc3339();
    let _ = write_record_atomic(&StatusRecord {
        workspace: ws.to_string(),
        state: "idle".into(),
        session_id: None,
        cwd: None,
        detail: None,
        last_error: Some(ToolError {
            tool: None,
            message: message.to_string(),
            ts: ts.clone(),
            count: 1,
            surfaced: true,
        }),
        prompt_after: None,
        ts,
    });
}

/// Snapshot of every tracked session's status — read by the popover on open so
/// it shows correct state even if it missed live events while hidden/reloaded.
/// A pending-prompt deadline that passed unwatched (e.g. the app wasn't running)
/// is promoted here, and persisted, so the row still reads `needs_you`.
#[tauri::command]
pub fn sessions_status_list() -> Vec<StatusRecord> {
    crate::log_invoke_debug!("sessions_status_list");
    let now = chrono::Utc::now();
    load_all()
        .into_iter()
        .map(|record| match promote_overdue(&record, now) {
            Some(promoted) => {
                let _ = write_record_atomic(&promoted);
                promoted
            }
            None => record,
        })
        .collect()
}

/// Clear a session's `last_error` (the dismissible failed-tool block in the
/// popover). Rewrites the record without it — so a reopened popover doesn't
/// re-show a dismissed error — and the watcher re-emits the change. No-op if the
/// record is missing or already clear.
#[tauri::command]
pub fn clear_session_error(workspace: String) {
    crate::log_invoke!("clear_session_error", workspace = %workspace);
    if !is_safe_workspace_id(&workspace) {
        return;
    }
    let Some(mut record) = read_record(&workspace) else {
        return;
    };
    if record.last_error.is_none() {
        return;
    }
    record.last_error = None;
    record.ts = chrono::Utc::now().to_rfc3339();
    let _ = write_record_atomic(&record);
}

/// Remove status files with no matching session record (e.g. left over from a
/// session torn down while mAIestro Code wasn't running). Run once at startup.
pub fn sweep_stale() {
    let live: std::collections::HashSet<String> =
        crate::sessions::load_all().into_iter().map(|s| s.id).collect();
    let Ok(entries) = std::fs::read_dir(status_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let stale = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|ws| !live.contains(ws))
            .unwrap_or(false);
        if stale {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Rescue sessions left stuck in `creating` by a spawn that never finished.
///
/// A fresh spawn's background task (`finish_spawn`) is the only thing that clears
/// the `creating` marker — on success (`clear_creating`) or failure
/// (`write_spawn_error`). If the app quits or crashes mid-spawn (plausible during
/// a long `post_spawn_commands` run), that task dies with its `JoinHandle`
/// dropped, so the marker is never cleared and the row shows "Creating…" forever
/// (`sweep_stale` keeps the file because the `Session` record still exists). On
/// the next launch the background task is gone for good, so any surviving
/// `creating` record is definitively orphaned: convert it into a surfaced spawn
/// error the popover shows on the row, so the user can tear the broken workspace
/// down and retry. Run once at startup, after `sweep_stale`.
pub fn reconcile_stale_creating() {
    let stuck: Vec<String> = load_all()
        .into_iter()
        .filter(|r| r.state == "creating")
        .map(|r| r.workspace)
        .collect();
    for ws in stuck {
        tracing::warn!(session = %ws, "found a session stuck in `creating` at startup; surfacing as a spawn error");
        write_spawn_error(
            &ws,
            "Spawn was interrupted before it finished (mAIestro Code quit or crashed mid-spawn). \
             Tear this workspace down and start it again.",
        );
    }
}

/// The workspace id a status file path corresponds to (its file stem).
fn workspace_of(path: &Path) -> Option<String> {
    if path.extension().is_none_or(|ext| ext != "json") {
        return None;
    }
    path.file_stem().and_then(|s| s.to_str()).map(|s| s.to_string())
}

/// Start watching `~/.maiestro/status/` and emit a `session-status` event to the
/// frontend on every change. The returned watcher must be kept alive for the app
/// lifetime (dropping it stops watching), so the caller stores it in app state.
pub fn start_watcher(app: tauri::AppHandle) -> notify::Result<notify::RecommendedWatcher> {
    use notify::{Event, EventKind, RecursiveMode, Watcher};
    use tauri::Emitter;

    let dir = status_dir();
    std::fs::create_dir_all(&dir).ok();

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        let Ok(event) = res else { return };
        // Ignore access/metadata-only events; we only care about content changes.
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        // macOS FSEvents coalesces and reports imprecise kinds, so decide by what's
        // actually on disk now rather than trusting the event kind: present →
        // forward its record; gone → synthesize `ended` so the row clears.
        for path in &event.paths {
            let Some(ws) = workspace_of(path) else { continue };
            match std::fs::read_to_string(path) {
                Ok(data) => {
                    if let Ok(record) = serde_json::from_str::<StatusRecord>(&data) {
                        if let Some(pending) = &record.prompt_after {
                            schedule_promotion(&record.workspace, &record.ts, &pending.deadline);
                        }
                        let _ = app.emit("session-status", &record);
                    }
                }
                Err(_) => {
                    let record = StatusRecord {
                        workspace: ws,
                        state: "ended".into(),
                        session_id: None,
                        cwd: None,
                        detail: None,
                        last_error: None,
                        prompt_after: None,
                        ts: chrono::Utc::now().to_rfc3339(),
                    };
                    let _ = app.emit("session-status", &record);
                }
            }
        }
    })?;

    watcher.watch(&dir, RecursiveMode::NonRecursive)?;
    Ok(watcher)
}

/// When a `prompt_after` deadline comes due, promote the record if no later hook
/// replaced it ([`promote_if_unchanged`]). FSEvents may report one write more
/// than once; the extra timers find the record already changed and do nothing.
fn schedule_promotion(ws: &str, ts: &str, deadline: &str) {
    let delay = chrono::DateTime::parse_from_rfc3339(deadline)
        .ok()
        .and_then(|d| (d.with_timezone(&chrono::Utc) - chrono::Utc::now()).to_std().ok())
        .unwrap_or_default();
    let (ws, ts) = (ws.to_string(), ts.to_string());
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(delay).await;
        promote_if_unchanged(&ws, &ts);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(tool: &str, count: u32, surfaced: bool) -> ToolError {
        ToolError {
            tool: Some(tool.into()),
            message: "boom".into(),
            ts: "t".into(),
            count,
            surfaced,
        }
    }

    fn fail(tool: &str) -> Option<(Option<String>, String)> {
        Some((Some(tool.into()), "boom".into()))
    }

    // A first failure is recorded but stays pending (hidden) — Claude may recover.
    #[test]
    fn first_failure_is_pending() {
        let e = next_last_error("tool_failed", None, fail("Bash"), "t").unwrap();
        assert_eq!(e.count, 1);
        assert!(!e.surfaced);
    }

    // A second consecutive failure of the *same* tool is persistent → surfaced.
    #[test]
    fn repeated_same_tool_surfaces() {
        let prior = Some(err("Bash", 1, false));
        let e = next_last_error("tool_failed", prior, fail("Bash"), "t").unwrap();
        assert_eq!(e.count, 2);
        assert!(e.surfaced);
    }

    // A failure of a *different* tool resets the count (not the same retry).
    #[test]
    fn different_tool_resets_count() {
        let prior = Some(err("Bash", 1, false));
        let e = next_last_error("tool_failed", prior, fail("WebFetch"), "t").unwrap();
        assert_eq!(e.count, 1);
        assert!(!e.surfaced);
    }

    // A tool succeeding after a pending failure clears it — Claude recovered.
    #[test]
    fn tool_ok_clears_pending() {
        let prior = Some(err("Bash", 1, false));
        assert!(next_last_error("tool_ok", prior, None, "t").is_none());
    }

    // A tool succeeding does NOT clear an already-surfaced error (user dismisses).
    #[test]
    fn tool_ok_keeps_surfaced() {
        let prior = Some(err("Bash", 2, true));
        assert!(next_last_error("tool_ok", prior, None, "t").is_some());
    }

    // Claude stopping with a pending error promotes it to surfaced.
    #[test]
    fn idle_surfaces_pending() {
        let prior = Some(err("Bash", 1, false));
        let e = next_last_error("idle", prior, None, "t").unwrap();
        assert!(e.surfaced);
    }

    // Real ids pass; path-traversal / separator / dot ids are rejected so a
    // crafted `--workspace` can't make status_path escape ~/.maiestro/status/.
    #[test]
    fn workspace_for_cwd_matches_the_containing_worktree() {
        let sessions = vec![
            ("8-a".to_string(), "/src/work-8-a/repo".to_string()),
            ("9-b".to_string(), "/src/work-9-b/repo".to_string()),
        ];
        assert_eq!(workspace_for_cwd("/src/work-8-a/repo", &sessions).as_deref(), Some("8-a"));
        assert_eq!(workspace_for_cwd("/src/work-9-b/repo/frontend/", &sessions).as_deref(), Some("9-b"));
        // A sibling whose name merely extends the worktree's is not inside it.
        assert_eq!(workspace_for_cwd("/src/work-8-a/repo-2", &sessions), None);
        assert_eq!(workspace_for_cwd("/elsewhere", &sessions), None);
    }

    /// On Windows a Codex `cwd` uses backslashes and any drive-letter case.
    #[cfg(windows)]
    #[test]
    fn workspace_for_cwd_matches_windows_paths() {
        let sessions = vec![("8-a".to_string(), r"C:\src\work-8-a\repo".to_string())];
        assert_eq!(workspace_for_cwd(r"c:\src\work-8-a\repo\frontend", &sessions).as_deref(), Some("8-a"));
        assert_eq!(workspace_for_cwd("C:/src/work-8-a/repo/", &sessions).as_deref(), Some("8-a"));
        assert_eq!(workspace_for_cwd(r"C:\src\work-8-a\repo-2", &sessions), None);
    }

    #[test]
    fn workspace_id_validation() {
        assert!(is_safe_workspace_id("8-surface-per-session"));
        assert!(is_safe_workspace_id("123"));
        assert!(!is_safe_workspace_id(""));
        assert!(!is_safe_workspace_id("."));
        assert!(!is_safe_workspace_id(".."));
        assert!(!is_safe_workspace_id("../../etc/passwd"));
        assert!(!is_safe_workspace_id("a/b"));
        assert!(!is_safe_workspace_id("a\\b"));
        assert!(!is_safe_workspace_id("a\0b"));
    }

    // Credentialed URLs in tool stderr are masked; ordinary text is untouched.
    #[test]
    fn redact_masks_url_userinfo() {
        assert_eq!(
            redact_secrets("curl https://user:tok3n@github.com/x failed"),
            "curl https://***@github.com/x failed"
        );
        assert_eq!(
            redact_secrets("connect to postgres://admin:s3cret@db:5432/app"),
            "connect to postgres://***@db:5432/app"
        );
        // No userinfo → unchanged; plain messages pass through verbatim.
        assert_eq!(redact_secrets("GET https://api.github.com/x 404"), "GET https://api.github.com/x 404");
        assert_eq!(redact_secrets("file not found: /tmp/x"), "file not found: /tmp/x");
    }

    // A new turn/session clears any error; other events carry it forward.
    #[test]
    fn prompt_clears_and_busy_carries() {
        let prior = Some(err("Bash", 1, false));
        assert!(next_last_error("prompt", prior.clone(), None, "t").is_none());
        assert!(next_last_error("running", prior.clone(), None, "t").is_none());
        assert!(next_last_error("busy", prior, None, "t").is_some());
    }

    // The hook verb → (state, detail) mapping. Working verbs all collapse to
    // `busy`; only `notification` reads the payload for its detail.
    #[test]
    fn resolve_state_maps_verbs() {
        let empty = serde_json::json!({});
        assert_eq!(resolve_state("running", &empty), ("running".into(), None));
        assert_eq!(resolve_state("prompt", &empty), ("busy".into(), None));
        assert_eq!(resolve_state("idle", &empty), ("idle".into(), None));
        assert_eq!(resolve_state("ended", &empty), ("ended".into(), None));
        // Unknown verb is recorded verbatim rather than guessed.
        assert_eq!(resolve_state("mystery", &empty), ("mystery".into(), None));
    }

    // Working verbs that carry a tool name surface it as detail.
    #[test]
    fn resolve_state_carries_tool_name() {
        let payload = serde_json::json!({ "tool_name": "Bash" });
        for verb in ["busy", "tool_failed", "tool_ok"] {
            let (state, detail) = resolve_state(verb, &payload);
            assert_eq!(state, "busy");
            assert_eq!(detail.as_deref(), Some("Bash"));
        }
    }

    // A notification with a message is needs_you + that message; an empty message
    // still flips to needs_you but with no detail.
    #[test]
    fn resolve_state_notification() {
        let with_msg = serde_json::json!({ "message": "Waiting on approval" });
        assert_eq!(
            resolve_state("notification", &with_msg),
            ("needs_you".into(), Some("Waiting on approval".into()))
        );
        let empty_msg = serde_json::json!({ "message": "   " });
        assert_eq!(resolve_state("notification", &empty_msg), ("needs_you".into(), None));
        // Codex's PermissionRequest: no message, but a tool name.
        let codex = serde_json::json!({ "hook_event_name": "PermissionRequest", "tool_name": "Bash", "tool_input": { "command": "rm -rf x" } });
        assert_eq!(
            resolve_state("notification", &codex),
            ("needs_you".into(), Some("Permission requested: `Bash`".into()))
        );
    }

    // ── Filesystem-level tests (MAIESTRO_HOME-injected temp root) ───────────────
    //
    // The pure-logic tests above cover `next_last_error`/`resolve_state`; these
    // drive the actual on-disk read-merge-write cycle through `TempHome`, so
    // nothing touches `~/.maiestro/status/`.

    use crate::testutil::TempHome;

    fn record(ws: &str, state: &str) -> StatusRecord {
        StatusRecord {
            workspace: ws.into(),
            state: state.into(),
            session_id: None,
            cwd: None,
            detail: None,
            last_error: None,
            prompt_after: None,
            ts: "t".into(),
        }
    }

    // A written record round-trips through the file and the temp root actually
    // holds the file (proving the MAIESTRO_HOME override reaches status_dir).
    #[test]
    fn write_then_read_roundtrips() {
        let home = TempHome::new();
        write_record_atomic(&record("8-add-foo", "busy")).unwrap();
        assert!(home.join("status/8-add-foo.json").exists());
        let got = read_record("8-add-foo").expect("record should read back");
        assert_eq!(got.state, "busy");
        assert!(!home.join("status/.8-add-foo.json.tmp").exists(), "temp must be renamed away");
    }

    // creating → clear_creating removes the file only while still `creating`.
    #[test]
    fn clear_creating_only_removes_creating() {
        let _home = TempHome::new();
        write_creating("9-bar");
        assert_eq!(read_record("9-bar").unwrap().state, "creating");
        clear_creating("9-bar");
        assert!(read_record("9-bar").is_none(), "creating marker should be removed");

        // A non-creating record is left untouched by clear_creating.
        write_record_atomic(&record("9-bar", "busy")).unwrap();
        clear_creating("9-bar");
        assert_eq!(read_record("9-bar").unwrap().state, "busy");
    }

    // clear_session_error rewrites the record without last_error; idempotent when
    // already clear or missing.
    #[test]
    fn clear_session_error_drops_last_error() {
        let _home = TempHome::new();
        let mut rec = record("10-baz", "idle");
        rec.last_error = Some(ToolError {
            tool: Some("Bash".into()),
            message: "boom".into(),
            ts: "t".into(),
            count: 2,
            surfaced: true,
        });
        write_record_atomic(&rec).unwrap();

        clear_session_error("10-baz".into());
        let got = read_record("10-baz").expect("record should survive");
        assert!(got.last_error.is_none(), "last_error should be cleared");
        assert_eq!(got.state, "idle", "state is preserved");

        // No-op paths: already-clear record and a missing one don't panic.
        clear_session_error("10-baz".into());
        clear_session_error("does-not-exist".into());
    }

    // write_spawn_error records a surfaced last_error the popover will show.
    #[test]
    fn spawn_error_is_surfaced() {
        let _home = TempHome::new();
        write_spawn_error("11-broken", "git worktree add failed");
        let got = read_record("11-broken").expect("record written");
        let err = got.last_error.expect("last_error present");
        assert!(err.surfaced, "spawn errors surface immediately");
        assert_eq!(err.message, "git worktree add failed");
    }

    // run_hook_cli end-to-end: the read-merge-write cycle preserves a pending
    // error across a carrying event, exactly as the hook helper does live.
    #[test]
    fn hook_cli_merges_prior_error_forward() {
        let _home = TempHome::new();
        // Seed a pending (unsurfaced) failure.
        let mut seed = record("12-merge", "busy");
        seed.last_error = Some(ToolError {
            tool: Some("Bash".into()),
            message: "boom".into(),
            ts: "t".into(),
            count: 1,
            surfaced: false,
        });
        write_record_atomic(&seed).unwrap();

        // A `busy` (PreToolUse) event carries the pending error forward.
        run_hook_cli(&["busy".into(), "--workspace".into(), "12-merge".into()]);
        let after = read_record("12-merge").expect("record after hook");
        assert!(after.last_error.is_some(), "pending error carried forward on busy");

        // A `prompt` (new turn) clears it.
        run_hook_cli(&["prompt".into(), "--workspace".into(), "12-merge".into()]);
        let cleared = read_record("12-merge").expect("record after prompt");
        assert!(cleared.last_error.is_none(), "new turn clears the error");
    }

    fn at(rfc3339: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(rfc3339).unwrap().with_timezone(&chrono::Utc)
    }

    /// A gated `run_command` waits its `WaitMsBeforeAsync` plus the grace (default
    /// when missing, capped when huge) and names the command; a file write or
    /// edit gets the short grace and names the tool.
    #[test]
    fn pending_prompt_deadlines_and_details() {
        use serde_json::json;
        let now = at("2026-10-02T12:00:00Z");
        let cmd = |args: serde_json::Value| json!({ "toolCall": { "name": "run_command", "args": args } });

        let p = pending_prompt(&cmd(json!({ "CommandLine": "curl -s https://example.com", "WaitMsBeforeAsync": 1000 })), now);
        assert_eq!(at(&p.deadline), at("2026-10-02T12:00:03Z"));
        assert_eq!(p.detail, "Run this command? `curl -s https://example.com`");

        let p = pending_prompt(&cmd(json!({ "CommandLine": "ls" })), now);
        assert_eq!(at(&p.deadline), at("2026-10-02T12:00:07Z"), "default wait");
        let p = pending_prompt(&cmd(json!({ "CommandLine": "ls", "WaitMsBeforeAsync": 600000 })), now);
        assert_eq!(at(&p.deadline), at("2026-10-02T12:01:02Z"), "capped wait");
        assert_eq!(pending_prompt(&cmd(json!({})), now).detail, "Run this command?");

        // Credentials are masked and a long command is shortened.
        let p = pending_prompt(&cmd(json!({ "CommandLine": "git push https://me:tok@github.com/o/r" })), now);
        assert_eq!(p.detail, "Run this command? `git push https://***@github.com/o/r`");
        let long = "x".repeat(300);
        let p = pending_prompt(&cmd(json!({ "CommandLine": long })), now);
        assert_eq!(p.detail, format!("Run this command? `{}…`", "x".repeat(119)));

        let edit = json!({ "toolCall": { "name": "write_to_file", "args": { "TargetFile": "/w/a.txt" } } });
        let p = pending_prompt(&edit, now);
        assert_eq!(at(&p.deadline), at("2026-10-02T12:00:03Z"));
        assert_eq!(p.detail, "Permission requested: `write_to_file`");
    }

    /// Only a record with a passed (or unreadable) deadline is promoted, to
    /// `needs_you` with the pending detail and the deadline cleared.
    #[test]
    fn promote_overdue_rule() {
        let now = at("2026-10-02T12:00:05Z");
        let mut r = record("208-x", "busy");
        assert!(promote_overdue(&r, now).is_none(), "nothing pending");

        r.prompt_after = Some(PendingPrompt { deadline: "2026-10-02T12:00:06Z".into(), detail: "Run this command? `ls`".into() });
        assert!(promote_overdue(&r, now).is_none(), "not due yet");

        r.prompt_after.as_mut().unwrap().deadline = "2026-10-02T12:00:05Z".into();
        let p = promote_overdue(&r, now).expect("due");
        assert_eq!(p.state, "needs_you");
        assert_eq!(p.detail.as_deref(), Some("Run this command? `ls`"));
        assert!(p.prompt_after.is_none());
        assert_eq!(p.workspace, "208-x");

        r.prompt_after.as_mut().unwrap().deadline = "garbage".into();
        assert!(promote_overdue(&r, now).is_some(), "an unreadable deadline counts as passed");
    }

    /// End to end on disk: a `gated` hook records a deadline, the next hook
    /// clears it, a stale promotion is a no-op, and an overdue record is
    /// promoted (and persisted) when the popover lists statuses.
    #[test]
    fn gated_hook_lifecycle_on_disk() {
        let _home = TempHome::new();
        run_hook_cli(&["gated".into(), "--workspace".into(), "208-x".into()]);
        let gated = read_record("208-x").expect("record after gated");
        assert_eq!(gated.state, "busy");
        assert!(gated.prompt_after.is_some(), "gated sets a deadline");

        run_hook_cli(&["tool_done".into(), "--workspace".into(), "208-x".into()]);
        let after = read_record("208-x").expect("record after tool_done");
        assert!(after.prompt_after.is_none(), "a later hook clears the deadline");

        // A timer armed for the gated record finds it replaced and does nothing.
        promote_if_unchanged("208-x", &gated.ts);
        assert_eq!(read_record("208-x").unwrap().state, "busy");

        let mut overdue = record("208-x", "busy");
        overdue.prompt_after = Some(PendingPrompt { deadline: "2020-01-01T00:00:00Z".into(), detail: "Run this command? `ls`".into() });
        write_record_atomic(&overdue).unwrap();
        let listed = sessions_status_list();
        assert_eq!(listed[0].state, "needs_you");
        assert_eq!(read_record("208-x").unwrap().state, "needs_you", "promotion is persisted");

        // promote_if_unchanged promotes a record still carrying its due deadline.
        write_record_atomic(&overdue).unwrap();
        promote_if_unchanged("208-x", "t");
        assert_eq!(read_record("208-x").unwrap().state, "needs_you");
    }

    /// Antigravity's payload-dependent verbs: the first model call of a turn is a
    /// fresh prompt and later ones are busy; a PostToolUse is ok unless it
    /// carries an error; Stop is idle. Other verbs pass through.
    #[test]
    fn normalize_verb_reads_the_antigravity_payload() {
        use serde_json::json;
        assert_eq!(normalize_verb("invocation", &json!({ "invocationNum": 0 })), "prompt");
        assert_eq!(normalize_verb("invocation", &json!({ "invocationNum": 2 })), "busy");
        assert_eq!(normalize_verb("invocation", &json!(null)), "prompt");
        assert_eq!(normalize_verb("tool_done", &json!({ "error": "" })), "tool_ok");
        assert_eq!(normalize_verb("tool_done", &json!({ "error": "exit status 1" })), "tool_failed");
        assert_eq!(normalize_verb("stop", &json!({ "error": "" })), "idle");
        assert_eq!(normalize_verb("busy", &json!({})), "busy");
        assert_eq!(normalize_verb("gated", &json!({})), "gated");
    }

    /// Antigravity's camelCase payload: the tool name comes from `toolCall.name`,
    /// an `ask_question` notification shows its question, and a failure message
    /// comes from `error`.
    #[test]
    fn resolve_state_reads_antigravity_payloads() {
        use serde_json::json;
        let post = json!({ "toolCall": { "name": "run_command", "args": {} }, "error": "exit status 1", "workspacePaths": ["/w"] });
        assert_eq!(resolve_state("tool_failed", &post), ("busy".into(), Some("run_command".into())));
        assert_eq!(extract_error_message(&post), "exit status 1");
        let ask = json!({ "toolCall": { "name": "ask_question", "args": { "questions": [{ "question": "Red or blue?", "options": ["Red", "Blue"] }] } } });
        assert_eq!(resolve_state("notification", &ask), ("needs_you".into(), Some("Red or blue?".into())));
        let gated = json!({ "toolCall": { "name": "run_command", "args": { "CommandLine": "ls" } } });
        assert_eq!(resolve_state("gated", &gated), ("busy".into(), Some("run_command".into())));
        let perm = json!({ "toolCall": { "name": "ask_permission", "args": {} } });
        assert_eq!(resolve_state("notification", &perm), ("needs_you".into(), Some("Permission requested: `ask_permission`".into())));
    }

    /// Copilot's camelCase payloads: `toolName` names the tool (a permission
    /// request reads as needs-you naming it), a failure's `error` string is the
    /// message, `sessionStart` is running, and `sessionId` is the session id.
    #[test]
    fn resolve_state_reads_copilot_payloads() {
        use serde_json::json;
        let perm = json!({ "sessionId": "s1", "timestamp": 1, "cwd": "/w", "hookName": "permissionRequest", "toolName": "bash", "toolInput": { "command": "rm x" }, "permissionSuggestions": [] });
        assert_eq!(resolve_state("notification", &perm), ("needs_you".into(), Some("Permission requested: `bash`".into())));
        let pre = json!({ "sessionId": "s1", "toolName": "view", "toolArgs": { "path": "a.txt" } });
        assert_eq!(resolve_state("busy", &pre), ("busy".into(), Some("view".into())));
        let failed = json!({ "sessionId": "s1", "toolName": "view", "toolArgs": {}, "error": "Path does not exist" });
        assert_eq!(resolve_state("tool_failed", &failed), ("busy".into(), Some("view".into())));
        assert_eq!(extract_error_message(&failed), "Path does not exist");
        assert_eq!(extract_error_message(&json!({ "error": { "message": "rate limited", "name": "Error" } })), "rate limited");
        assert_eq!(normalize_verb("session_start", &json!({ "source": "new" })), "running");
        assert_eq!(payload_session_id(&pre), Some("s1"));
        assert_eq!(resolve_state("error", &json!({})), ("busy".into(), None));
    }

    /// A `sessionStart` that lands after its own session's turn began is
    /// dropped; one from a new session (or onto an idle record) is not.
    #[test]
    fn copilot_late_session_start_keeps_the_working_state() {
        let _home = TempHome::new();
        let mut seed = record("203-late", "busy");
        seed.session_id = Some("s1".into());
        write_record_atomic(&seed).unwrap();
        let start = serde_json::json!({ "sessionId": "s1", "source": "new" });
        assert!(session_start_is_late(Some(&seed), &start));
        assert!(!session_start_is_late(Some(&seed), &serde_json::json!({ "sessionId": "s2" })), "a new session");
        let mut idle = seed.clone();
        idle.state = "idle".into();
        assert!(!session_start_is_late(Some(&idle), &start));
        assert!(!session_start_is_late(None, &start));

        // Without a payload the helper can't tell it's late, so it records it.
        run_hook_cli(&["session_start".into(), "--workspace".into(), "203-late".into()]);
        assert_eq!(read_record("203-late").unwrap().state, "running");
    }

    /// Copilot's `errorOccurred` surfaces a `last_error` at once, which the
    /// following `agentStop` (idle) carries and a new prompt clears.
    #[test]
    fn copilot_error_occurred_is_surfaced() {
        let _home = TempHome::new();
        write_record_atomic(&record("203-err", "busy")).unwrap();
        run_hook_cli(&["error".into(), "--workspace".into(), "203-err".into()]);
        let after = read_record("203-err").unwrap();
        let err = after.last_error.expect("error recorded");
        assert!(err.surfaced);
        assert_eq!(err.message, "The agent reported an error");
        run_hook_cli(&["idle".into(), "--workspace".into(), "203-err".into()]);
        assert!(read_record("203-err").unwrap().last_error.is_some_and(|e| e.surfaced));
        run_hook_cli(&["prompt".into(), "--workspace".into(), "203-err".into()]);
        assert!(read_record("203-err").unwrap().last_error.is_none());
    }

    /// A Stop that carried an error surfaces it right away, redacted.
    #[test]
    fn stop_error_is_surfaced() {
        let e = stop_error("fetch https://u:secret@host/x failed", "t");
        assert!(e.surfaced && e.tool.is_none());
        assert_eq!(e.message, "fetch https://***@host/x failed");
    }
}
