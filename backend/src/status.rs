//! Per-session live status: **busy / needs-you / idle**.
//!
//! mAIestro launches `claude` into VS Code/terminal and no longer owns its
//! stdio (see CLAUDE.md → "mAIestro launches sessions; it does not host them"),
//! so it can't read working/waiting state from the stream. Instead, each spawned
//! worktree gets Claude Code hooks (written by `spawn.rs`) that invoke this very
//! binary as `maiestro hook <state> --workspace <ws-id>`. The hook reads Claude's
//! event JSON on stdin, writes a small status record to `~/.maiestro/status/<ws-id>.json`,
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
    /// One of: running | busy | needs_you | idle | ended.
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
    /// RFC-3339 timestamp of the transition.
    pub ts: String,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

fn status_dir() -> PathBuf {
    home().join(".maiestro/status")
}

fn status_path(ws: &str) -> PathBuf {
    status_dir().join(format!("{ws}.json"))
}

// ── Hook CLI (`maiestro hook <state> --workspace <ws-id>`) ──────────────────────

/// Map the CLI state argument + the parsed hook payload into the record's
/// `state` and `detail`. `notification` is the only one that inspects the
/// payload to choose between waiting-on-permission and an idle nudge — both
/// surface as `needs_you`, differing only in `detail`.
fn resolve_state(arg: &str, payload: &serde_json::Value) -> (String, Option<String>) {
    match arg {
        "running" => ("running".into(), None),
        "busy" => {
            // PreToolUse carries the tool name; UserPromptSubmit does not.
            let detail = payload["tool_name"].as_str().map(|t| t.to_string());
            ("busy".into(), detail)
        }
        "notification" => {
            let msg = payload["message"].as_str().unwrap_or("").trim();
            let detail = if msg.is_empty() { None } else { Some(msg.to_string()) };
            ("needs_you".into(), detail)
        }
        "idle" => ("idle".into(), None),
        "ended" => ("ended".into(), None),
        // Unknown verb: record it verbatim rather than guessing.
        other => (other.into(), None),
    }
}

/// Entry point for `maiestro hook …`, dispatched from `main()` before Tauri
/// starts. Reads the hook payload from stdin, writes the status record, and
/// returns. Deliberately failure-tolerant: a hook must never block or crash the
/// user's Claude session, so every error is swallowed.
pub fn run_hook_cli(args: &[String]) {
    // args = ["<state>", "--workspace", "<ws-id>"] (order-tolerant for the flag).
    let state_arg = args.first().map(|s| s.as_str()).unwrap_or("");
    let workspace = args
        .iter()
        .position(|a| a == "--workspace")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_default();
    if state_arg.is_empty() || workspace.is_empty() {
        return;
    }

    // Best-effort stdin parse: Claude sends a JSON event, but we still record a
    // status even if it's empty or malformed.
    let mut buf = String::new();
    let _ = std::io::stdin().read_to_string(&mut buf);
    let payload: serde_json::Value = serde_json::from_str(&buf).unwrap_or(serde_json::Value::Null);

    let (state, detail) = resolve_state(state_arg, &payload);
    let record = StatusRecord {
        workspace: workspace.clone(),
        state,
        session_id: payload["session_id"].as_str().map(|s| s.to_string()),
        cwd: payload["cwd"].as_str().map(|s| s.to_string()),
        detail,
        ts: chrono::Utc::now().to_rfc3339(),
    };

    let _ = write_record_atomic(&record);
}

/// Write `~/.maiestro/status/<ws>.json` atomically (temp file + rename) so the
/// watcher never reads a half-written record.
fn write_record_atomic(record: &StatusRecord) -> std::io::Result<()> {
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

/// Snapshot of every tracked session's status — read by the popover on open so
/// it shows correct state even if it missed live events while hidden/reloaded.
#[tauri::command]
pub fn sessions_status_list() -> Vec<StatusRecord> {
    load_all()
}

/// Remove status files with no matching session record (e.g. left over from a
/// session torn down while mAIestro wasn't running). Run once at startup.
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
