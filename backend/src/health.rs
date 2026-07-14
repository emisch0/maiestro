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

#[derive(serde::Serialize, Clone, Copy, PartialEq)]
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
}

impl HealthCheck {
    fn new(id: &str, label: &str, status: HealthStatus, detail: impl Into<String>) -> Self {
        Self { id: id.into(), label: label.into(), status, detail: detail.into(), sub: Vec::new() }
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
pub async fn repo_health_check(repo: String) -> Result<HealthReport, String> {
    crate::log_invoke!("repo_health_check", repo = %repo);
    let settings = repo_settings::repo_settings_get(repo.clone())?;

    let checks = vec![
        check_cloned_repo(settings.cloned_repo_dir.as_deref()),
        check_cli("git", "Git available"),
        check_cli("claude", "Claude available"),
        check_github(&repo, settings.identity_id.as_deref()).await,
        check_editor(),
        check_env_files(settings.cloned_repo_dir.as_deref(), &settings.env_files),
    ];

    Ok(HealthReport { repo, checks })
}

/// `cloned_repo_dir` is set, exists, and is a valid git repository.
fn check_cloned_repo(cloned_repo_dir: Option<&str>) -> HealthCheck {
    let id = "cloned_repo";
    let label = "Cloned repo exists";
    let Some(dir) = cloned_repo_dir.filter(|d| !d.trim().is_empty()) else {
        return HealthCheck::new(id, label, HealthStatus::Fail, "No cloned_repo_dir configured");
    };
    let path = expand_tilde(dir);
    if !path.is_dir() {
        return HealthCheck::new(id, label, HealthStatus::Fail, format!("Not found: {}", path.display()));
    }
    // A directory alone isn't enough — confirm it's actually a git checkout.
    let is_git = crate::tools::command("git")
        .arg("-C")
        .arg(&path)
        .args(["rev-parse", "--git-dir"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if is_git {
        HealthCheck::new(id, label, HealthStatus::Pass, path.display().to_string())
    } else {
        HealthCheck::new(id, label, HealthStatus::Fail, format!("Not a git repository: {}", path.display()))
    }
}

/// A directly-invoked CLI (`git`, `claude`) resolves to a real file.
fn check_cli(tool: &str, label: &str) -> HealthCheck {
    let id = tool;
    match crate::tools::find_tool(tool) {
        Some(p) => HealthCheck::new(id, label, HealthStatus::Pass, p.display().to_string()),
        None => HealthCheck::new(id, label, HealthStatus::Fail, format!("`{tool}` not found on PATH")),
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
    let gh = match GitHub::for_identity(identity) {
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
    HealthCheck { id: id.into(), label: label.into(), status, detail, sub }
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
