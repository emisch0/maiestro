//! Subscription quota per agent account (issue #205): how much of each
//! rate-limit window the user's Claude, Codex and Copilot logins have used, for
//! the popover's quota strip. Read-only and soft-fail: an agent with no source,
//! or whose read fails, simply has no entry (the failure is logged at `warn`).
//! Antigravity has no reliable source and is not read. Quota belongs to the
//! ambient agent login, so it is per machine and agent (and, for Copilot, per
//! GitHub account), never per session. Details: `docs/provider-quotas.md`.
//!
//! - **Claude** — the only documented source is the `rate_limits` field of the
//!   status-line JSON. Each Claude worktree's `statusLine` runs this binary's
//!   hidden `statusline` subcommand ([`run_statusline_cli`]), which records it
//!   to `~/.maiestro/quota/claude.json` and then runs the user's own status line.
//!   [`start_watcher`] forwards each write to the popover.
//! - **Codex** — every running session logs `token_count` events carrying
//!   `rate_limits` to its rollout file under `~/.codex/sessions/`. A recently
//!   written one is read directly; otherwise one `account/rateLimits/read` call
//!   to `codex app-server` (cached [`CACHE_TTL`]).
//! - **Copilot** — `GET /copilot_internal/user` with the repo identity's own
//!   GitHub token, through the `GitHub::send` choke point (cached [`CACHE_TTL`]).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::agent::Agent;

/// One agent account's subscription quota.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderQuota {
    pub agent: Agent,
    /// The account, when the source names it (Copilot: the GitHub login). The
    /// popover shows one chip per agent + account.
    #[serde(default)]
    pub account: Option<String>,
    /// The plan, when the source names it (e.g. `plus`, `individual`).
    #[serde(default)]
    pub plan: Option<String>,
    pub windows: Vec<QuotaWindow>,
    /// When the numbers were read (Unix seconds).
    pub observed_at: i64,
}

/// One rate-limit window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaWindow {
    /// Short label: the window length (`5h`, `7d`, `30d`) or, for Copilot, the
    /// quota (`premium`, `plan`).
    pub label: String,
    /// 0–100; may exceed 100 once a limit is overrun.
    pub used_percent: f64,
    /// When the window resets (Unix seconds), if known.
    #[serde(default)]
    pub resets_at: Option<i64>,
    /// The window's full length in seconds, if known — with `resets_at`, how
    /// far through the window we are, for the popover's usage forecast.
    #[serde(default)]
    pub window_secs: Option<i64>,
}

/// How long a Codex app-server or Copilot reading is reused, so the popover's
/// polling doesn't spawn `codex` or call GitHub on every refresh.
const CACHE_TTL: Duration = Duration::from_secs(60);

/// A Codex rollout file written this recently belongs to a live session, so its
/// last `rate_limits` is current enough to show without asking the app-server.
const CODEX_LOG_FRESH: Duration = Duration::from_secs(10 * 60);

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

/// A window length in minutes as a short label: `300` → `5h`, `10080` → `7d`.
fn window_label(mins: u64) -> String {
    match mins {
        m if m >= 1440 && m % 1440 == 0 => format!("{}d", m / 1440),
        m if m >= 60 && m % 60 == 0 => format!("{}h", m / 60),
        m => format!("{m}m"),
    }
}

// ── Parsers (pure, unit-tested against captured payloads) ───────────────────────

/// Claude's status-line `rate_limits` → windows. `None` when it carries none
/// (API-key users, or before the session's first reply).
pub fn parse_claude_rate_limits(rate_limits: &serde_json::Value) -> Option<ProviderQuota> {
    let windows: Vec<QuotaWindow> = [("five_hour", "5h", 5 * 3600), ("seven_day", "7d", 7 * 86400)]
        .into_iter()
        .filter_map(|(key, label, secs)| {
            let w = &rate_limits[key];
            Some(QuotaWindow {
                label: label.into(),
                used_percent: w["used_percentage"].as_f64()?,
                resets_at: w["resets_at"].as_i64(),
                window_secs: Some(secs),
            })
        })
        .collect();
    (!windows.is_empty()).then(|| ProviderQuota {
        agent: Agent::Claude,
        account: None,
        plan: None,
        windows,
        observed_at: now_secs(),
    })
}

/// Codex rate limits → windows. Takes both spellings: the app-server's
/// `account/rateLimits/read` (`usedPercent`, `windowDurationMins`, `resetsAt`,
/// `planType`) and a rollout log's `token_count` event (`used_percent`,
/// `window_minutes`, `resets_at`, `plan_type`). `secondary` is often `null`.
pub fn parse_codex_rate_limits(rl: &serde_json::Value, observed_at: i64) -> Option<ProviderQuota> {
    let field = |v: &serde_json::Value, camel: &str, snake: &str| {
        let x = &v[camel];
        if x.is_null() { v[snake].clone() } else { x.clone() }
    };
    let windows: Vec<QuotaWindow> = ["primary", "secondary"]
        .into_iter()
        .filter_map(|key| {
            let w = &rl[key];
            let used = field(w, "usedPercent", "used_percent").as_f64()?;
            let mins = field(w, "windowDurationMins", "window_minutes").as_u64();
            Some(QuotaWindow {
                label: mins.map(window_label).unwrap_or_else(|| key.into()),
                used_percent: used,
                resets_at: field(w, "resetsAt", "resets_at").as_i64(),
                window_secs: mins.map(|m| m as i64 * 60),
            })
        })
        .collect();
    (!windows.is_empty()).then(|| ProviderQuota {
        agent: Agent::Codex,
        account: None,
        plan: field(rl, "planType", "plan_type").as_str().map(str::to_string),
        windows,
        observed_at,
    })
}

/// The `rate_limits` of the last `token_count` event in a Codex rollout log's
/// text, with the event's timestamp. Only the general `codex` limit (or one
/// with no id) counts; a per-model limit isn't the account's quota.
fn last_codex_log_rate_limits(log: &str) -> Option<(serde_json::Value, i64)> {
    log.lines().rev().filter(|l| l.contains("\"token_count\"")).find_map(|line| {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        let rl = &v["payload"]["rate_limits"];
        if v["payload"]["type"] != "token_count" || !rl.is_object() {
            return None;
        }
        if rl["limit_id"].as_str().is_some_and(|id| id != "codex") {
            return None;
        }
        let ts = v["timestamp"]
            .as_str()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map_or_else(now_secs, |t| t.timestamp());
        Some((rl.clone(), ts))
    })
}

/// Copilot's `copilot_internal/user` → windows: premium requests and, on plans
/// that meter it (Copilot Free), the `chat` allowance — labelled `plan`, as
/// Copilot CLI's own status line calls it ("Plan: 14/200"), since the CLI's
/// requests draw from it. A quota the plan doesn't have, or an unlimited one, is
/// skipped. Completions are IDE-only, not the CLI's.
pub fn parse_copilot_user(user: &serde_json::Value) -> Option<ProviderQuota> {
    let reset = user["quota_reset_date_utc"]
        .as_str()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok());
    let resets_at = reset.map(|t| t.timestamp());
    // The allowance is monthly: the window started a calendar month before the reset.
    let window_secs = reset.and_then(|t| {
        let start = t.checked_sub_months(chrono::Months::new(1))?;
        Some(t.timestamp() - start.timestamp())
    });
    let windows: Vec<QuotaWindow> = [("premium_interactions", "premium"), ("chat", "plan")]
        .into_iter()
        .filter_map(|(key, label)| {
            let q = &user["quota_snapshots"][key];
            if q["has_quota"] != true || q["unlimited"] == true {
                return None;
            }
            Some(QuotaWindow {
                label: label.into(),
                used_percent: 100.0 - q["percent_remaining"].as_f64()?,
                resets_at,
                window_secs,
            })
        })
        .collect();
    (!windows.is_empty()).then(|| ProviderQuota {
        agent: Agent::Copilot,
        account: user["login"].as_str().map(str::to_string),
        plan: user["copilot_plan"].as_str().map(str::to_string),
        windows,
        observed_at: now_secs(),
    })
}

// ── Claude: the `maiestro statusline` helper ───────────────────────────────────

fn quota_dir() -> PathBuf {
    crate::paths::maiestro_dir("quota")
}

fn claude_quota_path() -> PathBuf {
    quota_dir().join("claude.json")
}

/// How long the user's chained status line may run before it is killed and the
/// line left blank. Claude redraws often, so a hung command must not pile up.
const STATUSLINE_TIMEOUT: Duration = Duration::from_secs(5);

/// Entry point for `maiestro statusline --workspace <ws-id>`, dispatched from
/// `main()` before Tauri starts. Claude Code runs it as the worktree's
/// `statusLine` with the status-line JSON on stdin. Records `rate_limits` (when
/// present) as the Claude quota, then runs the status line the user would
/// otherwise see and passes its output through — none set prints nothing, as
/// before. Like the hook helper it must never block or crash the session, so
/// every failure is swallowed.
pub fn run_statusline_cli() {
    let mut input = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut input);
    let payload: serde_json::Value = serde_json::from_slice(&input).unwrap_or(serde_json::Value::Null);

    if let Some(quota) = parse_claude_rate_limits(&payload["rate_limits"]) {
        let _ = std::fs::create_dir_all(quota_dir());
        let _ = crate::paths::write_atomic(&claude_quota_path(), &serde_json::to_vec_pretty(&quota).unwrap_or_default());
    }

    let project_dir = payload["workspace"]["project_dir"]
        .as_str()
        .or(payload["cwd"].as_str())
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok());
    let Some(command) = project_dir
        .as_deref()
        .and_then(user_statusline)
        .and_then(|sl| sl["command"].as_str().map(str::to_string))
    else {
        return;
    };
    if let Some(out) = run_chained(&command, &input) {
        let _ = std::io::stdout().write_all(&out);
    }
}

/// The user's Claude settings directory: `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
fn claude_config_dir() -> PathBuf {
    match std::env::var("CLAUDE_CONFIG_DIR") {
        Ok(v) if !v.is_empty() => crate::paths::expand_tilde(&v),
        _ => crate::paths::home().join(".claude"),
    }
}

/// The status line the user would see in `project_dir` without ours: Claude's
/// precedence below `settings.local.json` (which holds ours) — the project's
/// `.claude/settings.json`, then the user's `settings.json`. Only a `command`
/// status line counts, and never one of ours (which would recurse).
pub fn user_statusline(project_dir: &Path) -> Option<serde_json::Value> {
    user_statusline_from(&[project_dir.join(".claude").join("settings.json"), claude_config_dir().join("settings.json")])
}

fn user_statusline_from(files: &[PathBuf]) -> Option<serde_json::Value> {
    files.iter().find_map(|f| {
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(f).ok()?).ok()?;
        let sl = v.get("statusLine")?;
        let cmd = sl["command"].as_str()?;
        (sl["type"] == "command" && !cmd.contains(" statusline --workspace ")).then(|| sl.clone())
    })
}

/// Run a status-line `command` through the shell with `input` on stdin, in the
/// current directory (Claude already started us in the session's), and return
/// its stdout — or `None` if it fails to start or outlives [`STATUSLINE_TIMEOUT`]
/// (then it is killed).
fn run_chained(command: &str, input: &[u8]) -> Option<Vec<u8>> {
    use std::process::{Command, Stdio};
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new("sh");
        c.arg("-c").arg(command);
        c
    };
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(command);
        c
    };
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    // Feed stdin and drain stdout on their own threads so a command that never
    // reads its input, or writes a lot, can't deadlock against us.
    let mut stdin = child.stdin.take()?;
    let input = input.to_vec();
    std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n @ 1..) = stdout.read(&mut buf) {
            if tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + STATUSLINE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    // Collect what it wrote. A background process the command left behind can
    // hold stdout open past its exit, so stop waiting for EOF shortly after.
    let grace = Instant::now() + Duration::from_millis(200);
    let mut out = Vec::new();
    while let Ok(chunk) = rx.recv_timeout(grace.saturating_duration_since(Instant::now())) {
        out.extend(chunk);
    }
    Some(out)
}

/// The last recorded Claude quota, if any. A corrupt file is logged and skipped.
fn read_claude() -> Option<ProviderQuota> {
    let path = claude_quota_path();
    let text = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str(&text) {
        Ok(q) => Some(q),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "unreadable Claude quota file");
            None
        }
    }
}

// ── Codex: the session logs, else the app-server ───────────────────────────────

/// Codex's home: `$CODEX_HOME`, else `~/.codex`.
fn codex_home() -> PathBuf {
    match std::env::var("CODEX_HOME") {
        Ok(v) if !v.is_empty() => crate::paths::expand_tilde(&v),
        _ => crate::paths::home().join(".codex"),
    }
}

/// The most recently written rollout log under `sessions/YYYY/MM/DD/`, looking
/// only at the newest few day folders (a session's file stays in the folder of
/// the day it started).
fn newest_codex_log(sessions: &Path) -> Option<(PathBuf, SystemTime)> {
    // Child folders, newest name first (the names are zero-padded dates).
    fn subdirs_desc(dir: &Path) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
            .unwrap_or_default();
        dirs.sort_unstable_by(|a, b| b.cmp(a));
        dirs
    }
    let days = subdirs_desc(sessions)
        .into_iter()
        .flat_map(|y| subdirs_desc(&y))
        .flat_map(|m| subdirs_desc(&m))
        .take(7);
    days.flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
        .max_by_key(|(t, _)| *t)
        .map(|(t, p)| (p, t))
}

/// The newest Codex rollout log's last quota, and whether the log is fresh
/// enough ([`CODEX_LOG_FRESH`]) to trust over the app-server. Reads only the
/// file's tail: a long session's log runs to megabytes.
fn read_codex_log() -> Option<(ProviderQuota, bool)> {
    let (path, modified) = newest_codex_log(&codex_home().join("sessions"))?;
    let mut file = std::fs::File::open(&path).ok()?;
    let len = file.metadata().ok()?.len();
    const TAIL: u64 = 512 * 1024;
    if len > TAIL {
        use std::io::Seek;
        file.seek(std::io::SeekFrom::Start(len - TAIL)).ok()?;
    }
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    let (rl, ts) = last_codex_log_rate_limits(&String::from_utf8_lossy(&buf))?;
    let quota = parse_codex_rate_limits(&rl, ts)?;
    let fresh = modified.elapsed().is_ok_and(|age| age < CODEX_LOG_FRESH);
    Some((quota, fresh))
}

/// One `account/rateLimits/read` through `codex app-server`, bounded at 10 s.
async fn read_codex_app_server() -> Result<ProviderQuota, String> {
    let reply = tokio::time::timeout(
        Duration::from_secs(10),
        crate::hooks::codex_app_server_request("account/rateLimits/read", serde_json::Value::Null, &[]),
    )
    .await
    .map_err(|_| "timed out".to_string())??;
    let result = &reply["result"];
    let rl = match &result["rateLimitsByLimitId"]["codex"] {
        v if v.is_object() => v,
        _ => &result["rateLimits"],
    };
    parse_codex_rate_limits(rl, now_secs()).ok_or_else(|| "no rate limits in the reply".to_string())
}

/// Readings reused for [`CACHE_TTL`], keyed by source (`codex`, `copilot:<identity>`).
fn cache() -> &'static Mutex<HashMap<String, (Instant, ProviderQuota)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, ProviderQuota)>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn cached(key: &str) -> Option<ProviderQuota> {
    let cache = cache().lock().unwrap_or_else(|e| e.into_inner());
    cache.get(key).filter(|(at, _)| at.elapsed() < CACHE_TTL).map(|(_, q)| q.clone())
}

fn remember(key: &str, quota: &ProviderQuota) {
    cache().lock().unwrap_or_else(|e| e.into_inner()).insert(key.to_string(), (Instant::now(), quota.clone()));
}

/// The Codex quota: a live session's log if one was written recently, else the
/// app-server (cached), else whatever the newest log last said.
async fn read_codex() -> Option<ProviderQuota> {
    let log = tokio::task::spawn_blocking(read_codex_log).await.ok().flatten();
    if let Some((quota, true)) = &log {
        return Some(quota.clone());
    }
    if let Some(q) = cached("codex") {
        return Some(q);
    }
    match read_codex_app_server().await {
        Ok(q) => {
            remember("codex", &q);
            Some(q)
        }
        Err(e) => {
            tracing::warn!(error = %e, "couldn't read the Codex quota from codex app-server");
            log.map(|(q, _)| q)
        }
    }
}

// ── Copilot: GitHub's `copilot_internal/user` ──────────────────────────────────

/// The Copilot quota of `identity_id`'s GitHub account (cached).
async fn read_copilot(identity_id: &str) -> Option<ProviderQuota> {
    let key = format!("copilot:{identity_id}");
    if let Some(q) = cached(&key) {
        return Some(q);
    }
    let result = async {
        let gh = crate::plugins::GitHub::for_identity(identity_id).await?;
        let user = gh.copilot_user().await?;
        parse_copilot_user(&user).ok_or_else(|| "no metered Copilot quota on this account".to_string())
    }
    .await;
    match result {
        Ok(q) => {
            remember(&key, &q);
            Some(q)
        }
        Err(e) => {
            tracing::warn!(identity = %identity_id, error = %e, "couldn't read the Copilot quota");
            None
        }
    }
}

// ── Command and watcher ────────────────────────────────────────────────────────

/// A visible work item's agent and repo — what the popover asks quota for.
#[derive(Debug, Clone, Deserialize)]
pub struct QuotaTarget {
    pub agent: Agent,
    pub repo: String,
}

/// The quota of every agent account behind `targets` (the popover's visible
/// work items): Claude's last recorded reading, Codex's, and Copilot's for each
/// distinct GitHub account the targets' repos use. An agent is read only when
/// a target uses it, so a Claude-only machine never runs `codex`. Antigravity
/// is never read. Soft-fail throughout: a failed read just leaves its entry out.
#[tauri::command]
pub async fn provider_quotas_list(targets: Vec<QuotaTarget>) -> Vec<ProviderQuota> {
    crate::log_invoke_debug!("provider_quotas_list", targets = targets.len());
    let uses = |a: Agent| targets.iter().any(|t| t.agent == a);
    let mut out = Vec::new();
    if uses(Agent::Claude) {
        out.extend(read_claude());
    }
    if uses(Agent::Codex) {
        out.extend(read_codex().await);
    }
    let mut identities: Vec<String> = targets
        .iter()
        .filter(|t| t.agent == Agent::Copilot)
        .filter_map(|t| crate::repo_settings::repo_settings_get(t.repo.clone()).ok()?.identity_id)
        .collect();
    identities.sort();
    identities.dedup();
    for identity in identities {
        if let Some(q) = read_copilot(&identity).await {
            // Two identities can be the same GitHub account.
            if !out.iter().any(|o: &ProviderQuota| o.agent == Agent::Copilot && o.account == q.account) {
                out.push(q);
            }
        }
    }
    out
}

/// Keeps the quota watcher alive in managed state (a newtype, so it can't
/// collide with the status watcher's).
pub struct QuotaWatcher(#[allow(dead_code)] pub Mutex<notify::RecommendedWatcher>);

/// Watch `~/.maiestro/quota/` and emit a `provider-quota` event with the new
/// reading whenever the status-line helper records one.
pub fn start_watcher(app: tauri::AppHandle) -> notify::Result<QuotaWatcher> {
    use notify::{Event, EventKind, RecursiveMode, Watcher};
    use tauri::Emitter;

    let dir = quota_dir();
    std::fs::create_dir_all(&dir).ok();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        let Ok(event) = res else { return };
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        // Decide by what's on disk (FSEvents kinds are imprecise); the temp file
        // of an atomic write isn't a reading.
        if event.paths.iter().any(|p| p == &claude_quota_path()) {
            if let Some(q) = read_claude() {
                let _ = app.emit("provider-quota", &q);
            }
        }
    })?;
    watcher.watch(&dir, RecursiveMode::NonRecursive)?;
    Ok(QuotaWatcher(Mutex::new(watcher)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_labels() {
        assert_eq!(window_label(300), "5h");
        assert_eq!(window_label(10080), "7d");
        assert_eq!(window_label(43200), "30d");
        assert_eq!(window_label(90), "90m");
    }

    /// A Pro/Max status-line payload's `rate_limits`, as Claude Code sends it.
    #[test]
    fn parses_claude_status_line_rate_limits() {
        let rl = serde_json::json!({
            "five_hour": { "used_percentage": 42.0, "resets_at": 1792865933 },
            "seven_day": { "used_percentage": 18.5, "resets_at": 1793000000 }
        });
        let q = parse_claude_rate_limits(&rl).unwrap();
        assert_eq!(q.agent, Agent::Claude);
        assert_eq!(q.windows.len(), 2);
        assert_eq!(q.windows[0], QuotaWindow { label: "5h".into(), used_percent: 42.0, resets_at: Some(1792865933), window_secs: Some(18000) });
        assert_eq!(q.windows[1].label, "7d");
        // A window Claude dropped (its reset passed) is just absent.
        let only_week = serde_json::json!({ "seven_day": { "used_percentage": 3, "resets_at": 1 } });
        assert_eq!(parse_claude_rate_limits(&only_week).unwrap().windows.len(), 1);
        // API-key sessions, or before the first reply: no field, no quota.
        assert!(parse_claude_rate_limits(&serde_json::Value::Null).is_none());
        assert!(parse_claude_rate_limits(&serde_json::json!({})).is_none());
    }

    /// `account/rateLimits/read` as codex-cli 0.157.0 answers it.
    #[test]
    fn parses_codex_app_server_reply() {
        let rl = serde_json::json!({
            "limitId": "codex",
            "primary": { "usedPercent": 6, "windowDurationMins": 43200, "resetsAt": 1792865933 },
            "secondary": null,
            "planType": "free"
        });
        let q = parse_codex_rate_limits(&rl, 100).unwrap();
        assert_eq!(q.plan.as_deref(), Some("free"));
        assert_eq!(q.observed_at, 100);
        assert_eq!(q.windows, vec![QuotaWindow { label: "30d".into(), used_percent: 6.0, resets_at: Some(1792865933), window_secs: Some(2592000) }]);

        let both = serde_json::json!({
            "primary": { "usedPercent": 42.5, "windowDurationMins": 300, "resetsAt": 1 },
            "secondary": { "usedPercent": 10, "windowDurationMins": 10080, "resetsAt": 2 }
        });
        let labels: Vec<_> = parse_codex_rate_limits(&both, 0).unwrap().windows.into_iter().map(|w| w.label).collect();
        assert_eq!(labels, ["5h", "7d"]);
        assert!(parse_codex_rate_limits(&serde_json::json!({ "primary": null, "secondary": null }), 0).is_none());
    }

    /// A rollout log: the last general-limit `token_count` wins; other events,
    /// per-model limits and `rate_limits: null` are skipped.
    #[test]
    fn reads_the_last_rate_limits_from_a_codex_log() {
        let log = [
            r#"{"timestamp":"2026-10-01T17:00:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{},"rate_limits":{"limit_id":"codex","primary":{"used_percent":4.0,"window_minutes":43200,"resets_at":1792865933},"secondary":null,"plan_type":"free"}}}"#,
            r#"{"timestamp":"2026-10-01T17:02:40.245Z","type":"event_msg","payload":{"type":"token_count","info":{},"rate_limits":{"limit_id":"codex","primary":{"used_percent":5.0,"window_minutes":43200,"resets_at":1792865933},"secondary":null,"plan_type":"free"}}}"#,
            r#"{"timestamp":"2026-10-01T17:03:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{},"rate_limits":{"limit_id":"codex_other","primary":{"used_percent":90.0,"window_minutes":300,"resets_at":1}}}}"#,
            r#"{"timestamp":"2026-10-01T17:04:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{},"rate_limits":null}}"#,
            r#"{"timestamp":"2026-10-01T17:05:00.000Z","type":"response_item","payload":{"type":"message"}}"#,
        ]
        .join("\n");
        let (rl, ts) = last_codex_log_rate_limits(&log).unwrap();
        assert_eq!(ts, chrono::DateTime::parse_from_rfc3339("2026-10-01T17:02:40.245Z").unwrap().timestamp());
        let q = parse_codex_rate_limits(&rl, ts).unwrap();
        assert_eq!(q.windows[0].used_percent, 5.0);
        assert_eq!(q.windows[0].label, "30d");
        assert_eq!(q.plan.as_deref(), Some("free"));
        assert!(last_codex_log_rate_limits("not json\n").is_none());
    }

    #[test]
    fn finds_the_newest_codex_log() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("2026/09/30");
        let new = dir.path().join("2026/10/01");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(old.join("rollout-a.jsonl"), "").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(new.join("rollout-b.jsonl"), "").unwrap();
        std::fs::write(new.join("notes.txt"), "").unwrap();
        let (path, _) = newest_codex_log(dir.path()).unwrap();
        assert!(path.ends_with("2026/10/01/rollout-b.jsonl"), "{path:?}");
        assert!(newest_codex_log(&dir.path().join("missing")).is_none());
    }

    /// `copilot_internal/user` for a free plan (premium not metered → skipped)
    /// and a paid one (premium metered, chat unlimited → skipped).
    #[test]
    fn parses_copilot_user() {
        let free = serde_json::json!({
            "login": "octo", "copilot_plan": "individual",
            "quota_snapshots": {
                "chat": { "percent_remaining": 93.1, "has_quota": true, "unlimited": false },
                "completions": { "percent_remaining": 100.0, "has_quota": true, "unlimited": false },
                "premium_interactions": { "percent_remaining": 0.0, "has_quota": false, "unlimited": false }
            },
            "quota_reset_date_utc": "2026-11-01T00:00:00.000Z"
        });
        let q = parse_copilot_user(&free).unwrap();
        assert_eq!(q.account.as_deref(), Some("octo"));
        assert_eq!(q.plan.as_deref(), Some("individual"));
        assert_eq!(q.windows.len(), 1);
        assert_eq!(q.windows[0].label, "plan", "Copilot CLI's own name for the free plan's allowance");
        assert!((q.windows[0].used_percent - 6.9).abs() < 1e-9);
        assert_eq!(q.windows[0].resets_at, Some(1793491200));
        assert_eq!(q.windows[0].window_secs, Some(31 * 86400), "October 1 → November 1");

        let pro = serde_json::json!({
            "login": "octo",
            "quota_snapshots": {
                "chat": { "percent_remaining": 100.0, "has_quota": true, "unlimited": true },
                "premium_interactions": { "percent_remaining": 25.0, "has_quota": true, "unlimited": false }
            }
        });
        let q = parse_copilot_user(&pro).unwrap();
        assert_eq!(q.windows, vec![QuotaWindow { label: "premium".into(), used_percent: 75.0, resets_at: None, window_secs: None }]);
        assert!(parse_copilot_user(&serde_json::json!({ "login": "x" })).is_none());
    }

    /// The chained status line: project settings win over user settings; a
    /// non-command status line or one of ours is ignored; none → nothing.
    #[test]
    fn resolves_the_users_status_line() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project.json");
        let user = dir.path().join("user.json");
        let missing = dir.path().join("missing.json");
        std::fs::write(&user, r#"{ "statusLine": { "type": "command", "command": "~/bin/sl", "padding": 1 } }"#).unwrap();
        std::fs::write(&project, r#"{ "statusLine": { "type": "command", "command": "./sl.sh" } }"#).unwrap();

        let sl = user_statusline_from(&[project.clone(), user.clone()]).unwrap();
        assert_eq!(sl["command"], "./sl.sh");
        let sl = user_statusline_from(&[missing.clone(), user.clone()]).unwrap();
        assert_eq!(sl["command"], "~/bin/sl");
        assert_eq!(sl["padding"], 1);

        std::fs::write(&project, r#"{ "statusLine": { "type": "command", "command": "'/x/maiestro' statusline --workspace '1-a'" } }"#).unwrap();
        assert_eq!(user_statusline_from(&[project.clone(), user.clone()]).unwrap()["command"], "~/bin/sl");
        assert!(user_statusline_from(std::slice::from_ref(&missing)).is_none());
        std::fs::write(&user, r#"{ "model": "opus" }"#).unwrap();
        assert!(user_statusline_from(&[missing, user]).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn chained_status_line_gets_stdin_and_is_bounded() {
        assert_eq!(run_chained("tr a-z A-Z", b"hello").unwrap(), b"HELLO");
        assert!(run_chained("exit 3", b"").unwrap().is_empty());
        // A lingering background child holding stdout doesn't hang us.
        let start = Instant::now();
        assert_eq!(run_chained("(sleep 30 &); echo hi", b"").unwrap(), b"hi\n");
        assert!(start.elapsed() < Duration::from_secs(10));
        let start = Instant::now();
        assert!(run_chained("sleep 30", b"").is_none());
        assert!(start.elapsed() < Duration::from_secs(10));
    }
}
