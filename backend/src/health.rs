//! Per-repo prerequisite health check (issue #93).
//!
//! `repo_health_check` runs a set of informational diagnostics for one tracked
//! repo — cloned checkout, the CLIs mAIestro invokes (`git`, `claude`, `code`),
//! the GitHub token *and the permissions it grants*, and the configured env
//! files — and returns a `HealthReport` the Settings window renders in a modal.
//!
//! The checks never mutate anything and never block spawning; they surface
//! likely problems early instead of letting them fail mid-spawn. The GitHub
//! permission check is the notable one: rather than performing a throwaway write
//! to prove write access, it *derives* the verdict from the token's granted
//! OAuth scopes (classic PATs, via `X-OAuth-Scopes`) or the repo's `permissions`
//! object (fine-grained tokens) — see the module's `github` sub-check.

use crate::paths::expand_tilde;
use crate::plugins::GitHub;
use crate::repo_settings;
use crate::tools::snippet;

#[derive(serde::Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Pass,
    Fail,
    Warn,
    Skipped,
}

#[derive(serde::Serialize)]
pub struct HealthCheck {
    /// Stable id (e.g. "cloned_repo", "github_token") for the frontend.
    pub id: String,
    pub label: String,
    pub status: HealthStatus,
    /// Human-readable context: a resolved path, a login, or an error message.
    pub detail: String,
    /// Nested checks, used by the GitHub check to break out token validity,
    /// repo read access, and write-permission verdict.
    pub sub: Vec<HealthCheck>,
    /// A suggested shell command to remediate a failure (e.g. a `git clone` when
    /// the repo isn't checked out). Rendered as a copyable monospace line; absent
    /// for checks with no obvious one-liner fix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

impl HealthCheck {
    fn new(id: &str, label: &str, status: HealthStatus, detail: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            status,
            detail: detail.into(),
            sub: Vec::new(),
            command: None,
        }
    }

    /// Attach a suggested remediation command (see `command`).
    fn with_command(mut self, command: impl Into<String>) -> Self {
        self.command = Some(command.into());
        self
    }
}

#[derive(serde::Serialize)]
pub struct HealthReport {
    pub repo: String,
    pub checks: Vec<HealthCheck>,
}

/// Run all prerequisite checks for `repo` ("owner/name") and return the report.
/// Only fails as a whole if the repo's settings file can't be loaded — every
/// individual prerequisite is reported as a check row, never an early error.
#[tauri::command]
pub async fn repo_health_check(window: tauri::Window, repo: String) -> Result<HealthReport, String> {
    crate::log_invoke!("repo_health_check", repo = %repo);
    let settings = repo_settings::repo_settings_get(repo.clone())?;

    // The model mAIestro's own drafting calls would use — the claude probe runs
    // against it so it doubles as a "is this model available?" check.
    let model = crate::prompts::model(&settings.prompt_model);
    // Run the probe inside the cloned repo when it exists (claude auth is global,
    // so cwd only needs to be a real directory); otherwise let it inherit ours.
    let claude_cwd = settings
        .cloned_repo_dir
        .as_deref()
        .map(expand_tilde)
        .filter(|p| p.is_dir());

    // Run the checks one at a time and emit each result the moment it lands, so
    // the popover streams rows (and shows a live spinner for the running one)
    // instead of waiting for the whole batch. `total` lets the UI stop spinning.
    let mut checks: Vec<HealthCheck> = Vec::new();
    let total = 6;
    // Each `step!` announces the check's title (so the spinner can name what's
    // running) *before* running it, then emits the result. The title here must
    // match the label the check function produces — the result event carries the
    // authoritative label, so any drift only shows for the in-flight moment.
    macro_rules! step {
        ($label:expr, $e:expr) => {{
            emit_running(&window, &repo, checks.len(), total, $label);
            let check = $e;
            emit_check(&window, &repo, checks.len(), total, &check);
            checks.push(check);
        }};
    }
    step!("Cloned repo exists", check_cloned_repo(&repo, settings.cloned_repo_dir.as_deref()).await);
    step!("Git available", check_cli("git", "Git available"));
    // The Claude probe returns one row ("Claude logged in") with the model check
    // nested as a sub — logged-in and model-available are distinct facts, and the
    // parent stays green (login) even when the model sub fails.
    step!("Claude logged in", check_claude(&repo, &model, claude_cwd.as_deref()).await);
    step!("GitHub token & permissions", check_github(&repo, settings.identity_id.as_deref()).await);
    step!("Session editor available", check_editor());
    step!("Configured env files exist", check_env_files(settings.cloned_repo_dir.as_deref(), &settings.env_files));

    Ok(HealthReport { repo, checks })
}

/// Announce the check about to run as a `health-check-running` event, so the
/// spinner can name what's executing. `index` is how many have already finished.
fn emit_running(window: &tauri::Window, repo: &str, index: usize, total: usize, label: impl AsRef<str>) {
    use tauri::Emitter;
    let _ = window.emit(
        "health-check-running",
        serde_json::json!({ "repo": repo, "index": index, "total": total, "label": label.as_ref() }),
    );
}

/// Log an external command the health check is about to run, at `info` so it's
/// visible in the default log (health checks are manual and infrequent, so this
/// doesn't spam). Never logs secrets — the commands here carry none.
fn log_command(repo: &str, command: &str) {
    tracing::info!(target: "health", repo = %repo, command = %command, "health-check command");
}

/// Emit one completed check to the popover as a `health-check` event. The UI
/// keys on `repo` (so a stale run's events are ignored) and uses `index`/`total`
/// to know when to stop the spinner. Best-effort — a failed emit never aborts the
/// remaining checks (the command's return value is the source of truth).
fn emit_check(window: &tauri::Window, repo: &str, index: usize, total: usize, check: &HealthCheck) {
    use tauri::Emitter;
    let _ = window.emit(
        "health-check",
        serde_json::json!({ "repo": repo, "index": index, "total": total, "check": check }),
    );
}

/// `cloned_repo_dir` is set, exists, and is a valid git repository whose `origin`
/// remote points at `repo` ("owner/name"). The remote-match is a sub-check, so a
/// checkout that exists but points at a *different* project fails distinctly from
/// a missing one.
async fn check_cloned_repo(repo: &str, cloned_repo_dir: Option<&str>) -> HealthCheck {
    let id = "cloned_repo";
    let label = "Cloned repo exists";
    let Some(dir) = cloned_repo_dir.filter(|d| !d.trim().is_empty()) else {
        // No target dir configured, so suggest cloning into the conventional
        // location (`~/src/<name>`, mAIestro's default cloned_repo_dir).
        let name = repo.rsplit('/').next().unwrap_or(repo);
        return HealthCheck::new(id, label, HealthStatus::Fail, "No cloned_repo_dir configured")
            .with_command(clone_command(repo, &format!("~/src/{name}")));
    };
    let path = expand_tilde(dir);
    if !path.is_dir() {
        // Not checked out yet — show how to clone it into the configured dir.
        return HealthCheck::new(id, label, HealthStatus::Fail, format!("Not found: {}", path.display()))
            .with_command(clone_command(repo, dir));
    }
    // A directory alone isn't enough — confirm it's actually a git checkout.
    log_command(repo, &format!("git -C {} rev-parse --git-dir", path.display()));
    let is_git = crate::tools::tokio_command("git")
        .arg("-C")
        .arg(&path)
        .args(["rev-parse", "--git-dir"])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !is_git {
        return HealthCheck::new(id, label, HealthStatus::Fail, format!("Not a git repository: {}", path.display()))
            .with_command(clone_command(repo, dir));
    }
    // Valid checkout: also confirm its `origin` points at this repo, so a
    // `cloned_repo_dir` aimed at the wrong project is caught here.
    let mut check = HealthCheck::new(id, label, HealthStatus::Pass, path.display().to_string());
    check.sub.push(check_remote_matches(repo, &path).await);
    check.status = rollup(&check.sub);
    check
}

/// The checkout's `origin` remote URL resolves to `repo` ("owner/name").
async fn check_remote_matches(repo: &str, path: &std::path::Path) -> HealthCheck {
    let id = "cloned_repo_remote";
    let label = "Remote matches this repo";
    log_command(repo, &format!("git -C {} remote get-url origin", path.display()));
    let out = crate::tools::tokio_command("git")
        .arg("-C")
        .arg(path)
        .args(["remote", "get-url", "origin"])
        .output()
        .await;
    let url = match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        // Non-zero exit is git's "No such remote 'origin'".
        Ok(_) => return HealthCheck::new(id, label, HealthStatus::Fail, "No `origin` remote configured"),
        Err(e) => return HealthCheck::new(id, label, HealthStatus::Warn, format!("Couldn't read origin: {e}")),
    };
    match remote_owner_name(&url) {
        Some(on) if on.eq_ignore_ascii_case(repo) => {
            HealthCheck::new(id, label, HealthStatus::Pass, format!("origin → {url}"))
        }
        Some(on) => HealthCheck::new(
            id,
            label,
            HealthStatus::Fail,
            format!("origin is {on}, expected {repo} ({url})"),
        ),
        None => HealthCheck::new(id, label, HealthStatus::Warn, format!("Unrecognized origin URL: {url}")),
    }
}

/// Extract "owner/name" from a git remote URL, covering the HTTPS
/// (`https://github.com/owner/name.git`), SSH (`ssh://git@github.com/owner/name`),
/// and scp-style (`git@github.com:owner/name.git`) forms by taking the last two
/// path segments. Returns `None` if fewer than two segments are present.
fn remote_owner_name(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let segments: Vec<&str> = trimmed
        .split(['/', ':'])
        .filter(|s| !s.is_empty())
        .collect();
    if segments.len() < 2 {
        return None;
    }
    let name = segments[segments.len() - 1];
    let owner = segments[segments.len() - 2];
    Some(format!("{owner}/{name}"))
}

/// A sample `git clone` command for `repo` ("owner/name") into `dir` (kept as the
/// user typed it, `~` and all, so it's a copy-paste-ready shell command). Uses the
/// SSH remote, matching how private-repo checkouts are typically cloned.
fn clone_command(repo: &str, dir: &str) -> String {
    format!("git clone git@github.com:{repo}.git {dir}")
}

/// A directly-invoked CLI (`git`, `claude`) resolves to a real file. Because a
/// configured `tool_paths` override is authoritative (see `tools::find_tool`), a
/// pin that doesn't exist is a hard **fail** naming the missing path — never a
/// silent fall-through to a different binary on PATH.
fn check_cli(tool: &str, label: &str) -> HealthCheck {
    let id = tool;
    match crate::tools::find_tool(tool) {
        Some(p) => HealthCheck::new(id, label, HealthStatus::Pass, p.display().to_string()),
        None => HealthCheck::new(id, label, HealthStatus::Fail, tool_not_found_detail(tool)),
    }
}

/// Explain a "not found" for a resolved tool: a configured-but-missing override
/// says so (and names the path); otherwise it's a plain not-on-PATH.
fn tool_not_found_detail(tool: &str) -> String {
    match crate::tools::stale_override(tool) {
        Some(stale) => format!("Configured path not found: {stale}"),
        None => format!("`{tool}` not found on PATH"),
    }
}

/// Probe Claude once and return the **logged in** (auth) check with a nested
/// **model available** sub-check, so the two failure modes are distinguishable
/// while staying visually grouped. Both come from a single `claude -p … --model
/// <model>` run:
/// - non-zero exit → auth failed (not logged in); model check is Skipped.
/// - `is_error` with a `401`/`403` status → auth failed; model Skipped.
/// - `is_error` with a *null* status (no HTTP request completed — claude bailed
///   out locally, e.g. "Not logged in · Please run /login") → auth failed; model
///   Skipped. This is the logged-out case: it exits 0 with an error envelope but
///   *no* status, so it must not be mistaken for a model problem.
/// - `is_error` with any other status (e.g. `404`) → auth *worked* (the API
///   answered), so login Passes and the **model** check Fails with the message.
/// - clean reply → both Pass.
///
/// A 404 authenticating-but-unknown-model is exactly the case worth separating:
/// the user is logged in fine, they just picked a `prompt_model` that isn't there.
///
/// If a `tool_paths.claude` override is configured but missing, resolution fails
/// outright (the override is authoritative — no PATH fallback), so this reports a
/// hard login **Fail** naming the pinned path rather than probing some other claude.
async fn check_claude(repo: &str, model: &str, cwd: Option<&std::path::Path>) -> HealthCheck {
    let login_id = "claude_login";
    let login_label = "Claude logged in";
    let model_id = "claude_model";
    let model_label = format!("Model `{model}` available");

    let model_skipped = |detail: &str| {
        HealthCheck::new(model_id, &model_label, HealthStatus::Skipped, detail)
    };
    // The model check hangs off the login check as a sub. The parent keeps the
    // *login* status (not a roll-up), so "Claude logged in" stays green even when
    // the model sub fails — you are logged in; you just picked a bad model.
    let nest = |mut login: HealthCheck, model: HealthCheck| {
        login.sub.push(model);
        login
    };

    // Resolve first so "not installed" (or a missing pinned override) is a
    // distinct, fast failure that names the offending path.
    let Some(bin) = crate::tools::find_tool("claude") else {
        return nest(
            HealthCheck::new(login_id, login_label, HealthStatus::Fail, tool_not_found_detail("claude")),
            model_skipped("claude not found"),
        );
    };
    let login_command = format!("{} login", bin.display());

    let mut cmd = crate::tools::tokio_command("claude");
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    // Keep it cheap and deterministic: no tools, JSON envelope (so we can read
    // `is_error`/`api_error_status`), a one-word reply.
    cmd.args([
        "-p",
        "Reply with exactly: ok",
        "--model",
        model,
        "--output-format",
        "json",
        "--tools",
        "",
    ]);
    log_command(repo, &format!("claude -p 'Reply with exactly: ok' --model {model} --output-format json --tools ''"));
    // Kill the probe if the timeout fires so it doesn't linger in the background.
    cmd.kill_on_drop(true);
    let run = tokio::time::timeout(std::time::Duration::from_secs(60), cmd.output()).await;

    let output = match run {
        Err(_) => {
            return nest(
                HealthCheck::new(login_id, login_label, HealthStatus::Warn, "claude timed out after 60s"),
                model_skipped("claude timed out"),
            );
        }
        Ok(Err(e)) => {
            return nest(
                HealthCheck::new(login_id, login_label, HealthStatus::Fail, format!("Couldn't run claude: {e}")),
                model_skipped("claude couldn't run"),
            );
        }
        Ok(Ok(o)) => o,
    };

    // The JSON envelope is authoritative, so parse it *before* looking at the
    // exit code: claude exits non-zero on an API error too (a bad model is exit 1
    // with `is_error`/`api_error_status: 404`), so a non-zero exit alone does NOT
    // mean "not logged in". Only a *missing* envelope falls back to the exit code.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let Ok(env) = serde_json::from_str::<serde_json::Value>(stdout.trim()) else {
        // No envelope to interpret. A non-zero exit here is the genuine
        // couldn't-run / not-logged-in case; a 0 exit means it ran but we can't
        // judge the model.
        if output.status.success() {
            return nest(
                HealthCheck::new(login_id, login_label, HealthStatus::Pass, "Logged in"),
                model_skipped("Couldn't parse claude output"),
            );
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = if stderr.trim().is_empty() { snippet(&stdout) } else { snippet(&stderr) };
        return nest(
            HealthCheck::new(login_id, login_label, HealthStatus::Fail, format!("Likely not logged in: {detail}"))
                .with_command(login_command),
            model_skipped("Claude not logged in"),
        );
    };

    classify_claude_envelope(&env, model, &login_command)
}

/// Turn a *parsed* claude result envelope into the nested login/model check.
/// Pure (no I/O) so the auth-vs-model decision — the part that actually had the
/// logged-out bug — is unit-tested directly. See `check_claude` for the mapping.
fn classify_claude_envelope(env: &serde_json::Value, model: &str, login_command: &str) -> HealthCheck {
    let login_id = "claude_login";
    let login_label = "Claude logged in";
    let model_id = "claude_model";
    let model_label = format!("Model `{model}` available");
    let model_skipped =
        |detail: &str| HealthCheck::new(model_id, &model_label, HealthStatus::Skipped, detail);
    let nest = |mut login: HealthCheck, model: HealthCheck| {
        login.sub.push(model);
        login
    };
    let login_fail = |detail: String| {
        nest(
            HealthCheck::new(login_id, login_label, HealthStatus::Fail, detail)
                .with_command(login_command.to_string()),
            model_skipped("Claude not logged in"),
        )
    };

    if !env["is_error"].as_bool().unwrap_or(false) {
        return nest(
            HealthCheck::new(login_id, login_label, HealthStatus::Pass, "Logged in"),
            HealthCheck::new(model_id, &model_label, HealthStatus::Pass, format!("`{model}` responded")),
        );
    }

    // An error envelope. `api_error_status` is the HTTP status of an *actual* API
    // response, so it tells auth from model — but only when it's present:
    // - 401/403: credentials were rejected → login Fails, model Skipped.
    // - a *null* status: no HTTP request ever completed; claude bailed out
    //   locally *before* authenticating (e.g. "Not logged in · Please run
    //   /login", or an offline/network error). We can't conclude auth worked, so
    //   this is a login Fail, not a model problem. This is the logged-out case.
    // - anything else (notably 404): the request authenticated but the model was
    //   the problem → login Passes, model Fails with the message.
    let status = env["api_error_status"].as_u64();
    let message = snippet(env["result"].as_str().unwrap_or(""));
    match status {
        Some(401) | Some(403) => login_fail(format!("Not authorized: {message}")),
        None => login_fail(format!("Likely not logged in: {message}")),
        Some(_) => nest(
            HealthCheck::new(login_id, login_label, HealthStatus::Pass, "Logged in"),
            HealthCheck::new(model_id, &model_label, HealthStatus::Fail, message),
        ),
    }
}

/// The session editor. Today mAIestro always launches VS Code (`open_vscode`),
/// preferring the `code` CLI and falling back to the app bundle — so mirror that:
/// pass if the `code` CLI resolves, warn (with the fallback still viable) if not.
fn check_editor() -> HealthCheck {
    let id = "editor";
    let label = "Session editor available";
    match crate::tools::find_tool("code") {
        Some(p) => HealthCheck::new(id, label, HealthStatus::Pass, p.display().to_string()),
        // A configured-but-missing `code` pin is authoritative — fail naming it,
        // rather than falling back to launching VS Code.app via `open -a`.
        None if crate::tools::stale_override("code").is_some() => {
            HealthCheck::new(id, label, HealthStatus::Fail, tool_not_found_detail("code"))
        }
        None if std::path::Path::new("/Applications/Visual Studio Code.app").is_dir() => HealthCheck::new(
            id,
            label,
            HealthStatus::Warn,
            "`code` CLI not found; will fall back to launching Visual Studio Code.app",
        ),
        None => HealthCheck::new(id, label, HealthStatus::Fail, "Visual Studio Code not found"),
    }
}

/// Every configured env file (resolved relative to `cloned_repo_dir`, as the
/// spawn path copies them) exists. Pass with none configured; warn listing any
/// missing.
fn check_env_files(cloned_repo_dir: Option<&str>, env_files: &[String]) -> HealthCheck {
    let id = "env_files";
    let label = "Configured env files exist";
    if env_files.is_empty() {
        return HealthCheck::new(id, label, HealthStatus::Pass, "None configured");
    }
    let base_dir = cloned_repo_dir.map(expand_tilde);
    let missing: Vec<&String> = env_files
        .iter()
        .filter(|rel| {
            let path = match &base_dir {
                Some(base) => base.join(rel.as_str()),
                None => expand_tilde(rel),
            };
            !path.exists()
        })
        .collect();
    if missing.is_empty() {
        HealthCheck::new(id, label, HealthStatus::Pass, format!("{} present", env_files.len()))
    } else {
        let list = missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
        HealthCheck::new(id, label, HealthStatus::Warn, format!("Missing: {list}"))
    }
}

/// The GitHub token check: validity + read access + a derived write-permission
/// verdict, as three sub-checks under one parent. Uses the repo's assigned
/// identity token; skipped entirely if no identity is assigned. Never mutates
/// the repo — write access is inferred, not exercised (see #93).
async fn check_github(repo: &str, identity_id: Option<&str>) -> HealthCheck {
    let id = "github_token";
    let label = "GitHub token & permissions";

    let Some(identity) = identity_id.filter(|i| !i.trim().is_empty()) else {
        return HealthCheck::new(id, label, HealthStatus::Skipped, "No identity assigned to this repo");
    };
    let gh = match GitHub::for_identity(identity).await {
        Ok(gh) => gh,
        Err(e) => return HealthCheck::new(id, label, HealthStatus::Fail, e),
    };

    // 1. Token validity + scopes.
    let token_info = match gh.check_token().await {
        Ok(info) => info,
        Err(e) => {
            let mut parent = HealthCheck::new(id, label, HealthStatus::Fail, "Token check failed");
            parent.sub.push(HealthCheck::new("token_valid", "Token valid", HealthStatus::Fail, e));
            return parent;
        }
    };
    let mut sub = vec![HealthCheck::new(
        "token_valid",
        "Token valid",
        HealthStatus::Pass,
        format!("Authenticated as {}", token_info.login),
    )];

    // 2. Repo read access — also the source of the `permissions` object used to
    //    judge write access for fine-grained tokens.
    let repo_json = gh.repo(repo).await;
    let (read_ok, repo_json) = match repo_json {
        Ok(v) => {
            sub.push(HealthCheck::new(
                "repo_read",
                "Repo readable",
                HealthStatus::Pass,
                format!("Read access to {repo}"),
            ));
            (true, Some(v))
        }
        Err(e) => {
            sub.push(HealthCheck::new("repo_read", "Repo readable", HealthStatus::Fail, e));
            (false, None)
        }
    };

    // 3. Write-permission verdict — derived, never exercised.
    sub.push(write_permission_check(&token_info.scopes, repo_json.as_ref(), read_ok));

    let status = rollup(&sub);
    let detail = format!("Identity: {identity}");
    HealthCheck { id: id.into(), label: label.into(), status, detail, sub, command: None }
}

/// Derive a write-permission verdict without mutating the repo:
/// - Classic PAT (non-empty scopes): needs `repo` (private) or at least
///   `public_repo` (public) — mAIestro creates issues/PRs and merges.
/// - Fine-grained PAT (empty scopes): read the repo's `permissions.push` flag.
fn write_permission_check(
    scopes: &[String],
    repo_json: Option<&serde_json::Value>,
    read_ok: bool,
) -> HealthCheck {
    let id = "repo_write";
    let label = "Write access (issues, PRs)";
    let has = |s: &str| scopes.iter().any(|x| x == s);

    if !scopes.is_empty() {
        // Classic PAT: the scope list is authoritative.
        if has("repo") {
            return HealthCheck::new(id, label, HealthStatus::Pass, "Classic token has `repo` scope");
        }
        let is_public = repo_json
            .and_then(|v| v["private"].as_bool())
            .map(|private| !private)
            .unwrap_or(false);
        if is_public && has("public_repo") {
            return HealthCheck::new(
                id,
                label,
                HealthStatus::Pass,
                "Classic token has `public_repo` scope (public repo)",
            );
        }
        return HealthCheck::new(
            id,
            label,
            HealthStatus::Fail,
            format!("Token missing `repo` scope (has: {})", scopes.join(", ")),
        );
    }

    // Fine-grained token (or GitHub App): infer from the repo `permissions`.
    let Some(v) = repo_json else {
        let detail = if read_ok {
            "Could not read repo permissions"
        } else {
            "Repo not readable, so write access can't be determined"
        };
        return HealthCheck::new(id, label, HealthStatus::Warn, detail);
    };
    match v["permissions"]["push"].as_bool() {
        Some(true) => HealthCheck::new(id, label, HealthStatus::Pass, "Fine-grained token can push to this repo"),
        Some(false) => HealthCheck::new(
            id,
            label,
            HealthStatus::Fail,
            "Token can read but cannot push to this repo (needs Contents/Issues/Pull requests: write)",
        ),
        None => HealthCheck::new(id, label, HealthStatus::Warn, "Repo permissions not reported by GitHub"),
    }
}

/// Worst-case roll-up of a group's sub-checks into the parent status:
/// any Fail → Fail; else any Warn → Warn; else any Skipped → Skipped; else Pass.
fn rollup(sub: &[HealthCheck]) -> HealthStatus {
    if sub.iter().any(|c| c.status == HealthStatus::Fail) {
        HealthStatus::Fail
    } else if sub.iter().any(|c| c.status == HealthStatus::Warn) {
        HealthStatus::Warn
    } else if sub.iter().any(|c| c.status == HealthStatus::Skipped) {
        HealthStatus::Skipped
    } else {
        HealthStatus::Pass
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_owner_name_parses_all_url_forms() {
        let cases = [
            "https://github.com/owner/name.git",
            "https://github.com/owner/name",
            "git@github.com:owner/name.git",
            "git@github.com:owner/name",
            "ssh://git@github.com/owner/name.git",
            "https://github.com/owner/name/",
        ];
        for url in cases {
            assert_eq!(remote_owner_name(url).as_deref(), Some("owner/name"), "url: {url}");
        }
    }

    #[test]
    fn clone_command_uses_ssh_remote_and_keeps_tilde_dir() {
        assert_eq!(
            clone_command("owner/name", "~/src/name"),
            "git clone git@github.com:owner/name.git ~/src/name"
        );
    }

    #[test]
    fn remote_owner_name_rejects_too_few_segments() {
        assert_eq!(remote_owner_name("name"), None);
        assert_eq!(remote_owner_name(""), None);
    }

    // Pull the nested model sub-check (there is always exactly one).
    fn model_sub(login: &HealthCheck) -> &HealthCheck {
        &login.sub[0]
    }

    #[test]
    fn classify_logged_out_envelope_fails_login_not_model() {
        // The real logged-out envelope: exits with an error but *no* HTTP status,
        // because claude bailed out before authenticating. Regression guard — this
        // used to be misread as "auth worked, model broken" and left login green.
        let env = serde_json::json!({
            "is_error": true,
            "api_error_status": null,
            "result": "Not logged in · Please run /login",
        });
        let login = classify_claude_envelope(&env, "haiku", "claude login");
        assert_eq!(login.status, HealthStatus::Fail);
        assert_eq!(login.command.as_deref(), Some("claude login"));
        assert_eq!(model_sub(&login).status, HealthStatus::Skipped);
    }

    #[test]
    fn classify_401_fails_login_and_skips_model() {
        let env = serde_json::json!({
            "is_error": true,
            "api_error_status": 401,
            "result": "Unauthorized",
        });
        let login = classify_claude_envelope(&env, "haiku", "claude login");
        assert_eq!(login.status, HealthStatus::Fail);
        assert_eq!(model_sub(&login).status, HealthStatus::Skipped);
    }

    #[test]
    fn classify_404_passes_login_but_fails_model() {
        // Authenticated fine, just an unknown model: login stays green, model red.
        let env = serde_json::json!({
            "is_error": true,
            "api_error_status": 404,
            "result": "model: nonesuch not found",
        });
        let login = classify_claude_envelope(&env, "nonesuch", "claude login");
        assert_eq!(login.status, HealthStatus::Pass);
        assert_eq!(model_sub(&login).status, HealthStatus::Fail);
    }

    #[test]
    fn classify_clean_reply_passes_both() {
        let env = serde_json::json!({
            "is_error": false,
            "api_error_status": null,
            "result": "ok",
        });
        let login = classify_claude_envelope(&env, "haiku", "claude login");
        assert_eq!(login.status, HealthStatus::Pass);
        assert_eq!(model_sub(&login).status, HealthStatus::Pass);
    }

    #[test]
    fn remote_owner_name_is_used_case_insensitively() {
        // The comparison at the call site is case-insensitive; the parser itself
        // preserves case, so callers must use eq_ignore_ascii_case.
        let parsed = remote_owner_name("https://github.com/Owner/Name.git").unwrap();
        assert!(parsed.eq_ignore_ascii_case("owner/name"));
    }
}
