//! Spawn core + teardown: create (or reopen) a git worktree + VS Code workspace
//! for a GitHub issue, and later tear it down.
//!
//! GitHub is reached via the REST API under the repo's identity (no `gh`), env
//! files come from the repo's settings, and the editor is launched via `open -a`
//! (no constructed env). The pieces this orchestrates live in focused modules
//! (issue #99): `theming`, `hooks`, `editor`, `drafting`, `pr`, plus the shared
//! `gitops` / `naming` / `repo_context` helpers.

use std::path::{Path, PathBuf};

use crate::drafting::{resolve_draft, ClaudeActivity, DraftStep};
use crate::editor::{
    close_editor_window, open_vscode, probe_editor_window, window_marker, worktree_in_use,
    write_vscode_files, WinProbe,
};
use crate::gitops::{git, git_net, local_branch_exists};
use crate::hooks::{reconcile_session_hooks, write_claude_hooks};
use crate::naming::{default_short_title, slugify};
use crate::paths::expand_tilde;
use crate::plugins::GitHub;
use crate::repo_context::{repo_context, validated_cloned_repo};
use crate::theming::pick_theme;
use crate::tools::snippet;

// ── Command ─────────────────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct SpawnResult {
    /// The workspace/session id (= `Session.id`), so the frontend can match the
    /// row this spawn created and track its `creating` status.
    pub session_id: String,
    pub work_dir: String,
    pub branch: String,
    pub issue_url: String,
    /// True when an existing workspace was reused rather than created.
    pub reused: bool,
    /// Non-fatal warnings raised during the *synchronous* phase. On a fresh
    /// spawn the heavy work now runs in the background, so its warnings are
    /// logged there rather than returned here.
    pub warnings: Vec<String>,
}

/// Result of `create_issue_and_spawn`: either the spawn went through, or Claude
/// couldn't turn the idea into a clear issue and we're asking the user whether
/// to create one from their raw text anyway.
#[derive(serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CreateAndSpawnOutcome {
    Spawned(SpawnResult),
    NeedsConfirmation { message: String },
}

/// Result of `create_issue`: either the issue was opened (no workspace spawned),
/// or Claude couldn't turn the idea into a clear issue and we're asking the user
/// whether to create one from their raw text anyway.
#[derive(serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CreateIssueOutcome {
    Created {
        number: u64,
        issue_url: String,
        warnings: Vec<String>,
    },
    NeedsConfirmation { message: String },
}

/// The decisions a spawn needs once the issue is known: the (possibly edited)
/// short label that drives the slug, and the chosen theming. Slugs and the
/// session title are derived from these so the preview and the spawn agree.
struct SpawnDecision<'a> {
    repo: &'a str,
    issue_number: u64,
    issue_url: &'a str,
    default_branch: &'a str,
    short_label: &'a str,
    color: &'a str,
    emoji: &'a str,
    force_new: bool,
}

/// Everything the background phase of a fresh spawn needs, owned so it can move
/// into the `tokio::spawn`ed task that builds the worktree after `do_spawn` has
/// already returned. See `finish_spawn`.
struct SpawnBg {
    gh: GitHub,
    cloned_repo: PathBuf,
    work_dir: PathBuf,
    work_parent: String,
    branch: String,
    workspace: String,
    session_title: String,
    color: String,
    default_branch: String,
    repo: String,
    issue_number: u64,
    env_files: Vec<String>,
    post_spawn_commands: Vec<String>,
}

/// Core worktree + session creation, shared by every spawn path. Resolves the
/// repo's settings/identity/cloned repo itself; the caller supplies the issue facts
/// and the (reviewed) label/theming. The slug is `<n>-<slug(short_label)>`.
///
/// Two-phase for a fresh spawn: this fast synchronous phase resolves the final
/// workspace id, records the `Session` row, and marks it `creating` (so the
/// popover shows the row with a "Creating…" pill immediately), then hands the
/// slow work — worktree add, env copy, GitHub assign/comment, post-spawn
/// commands, editor launch — to a background task and returns at once. Reopening
/// an existing worktree stays fully synchronous (it's near-instant).
#[tracing::instrument(skip_all, fields(session = tracing::field::Empty))]
async fn do_spawn(d: SpawnDecision<'_>) -> Result<SpawnResult, String> {
    let SpawnDecision { repo, issue_number, issue_url, default_branch, short_label, color, emoji, force_new } = d;

    let (settings, gh) = repo_context(repo).await?;
    let cloned_repo = validated_cloned_repo(&settings)?;
    let repo_name = repo.split('/').next_back().unwrap_or(repo).to_string();

    let short_label = {
        let t = short_label.trim();
        if t.is_empty() { default_short_title("") } else { t.to_string() }
    };

    // Worktree location prefix: the full path is `<prefix><workspace>/<repo>`
    // (string concat — the trailing `work-` is part of the dir name). Unset
    // falls back to the schema default.
    let worktree_prefix = effective_worktree_prefix(settings.worktree_prefix.as_deref());
    let worktree_dir = |workspace: &str| expand_tilde(&format!("{worktree_prefix}{workspace}")).join(&repo_name);

    // Workspace name: "<n>-<slug>".
    let prefix = format!("#{issue_number} — ");
    let base_workspace = format!("{issue_number}-{}", slugify(&short_label, 25));
    let base_branch = format!("feature/{base_workspace}");
    let base_dir = worktree_dir(&base_workspace);
    // Tag this span (and so every log line it emits) with the workspace session id.
    tracing::Span::current().record("session", base_workspace.as_str());

    // Reuse an existing workspace by default; --new forces a fresh one.
    if !force_new && base_dir.is_dir() {
        // Reopening doesn't rewrite hooks, so heal a stale binary path here too
        // (without waiting for the next startup reconcile).
        reconcile_session_hooks(&base_dir, &base_workspace);
        open_vscode(&base_dir)?;
        tracing::info!(repo = %repo, issue = issue_number, branch = %base_branch, reused = true, "spawned workspace");
        return Ok(SpawnResult {
            session_id: base_workspace,
            work_dir: base_dir.display().to_string(),
            branch: base_branch,
            issue_url: issue_url.to_string(),
            reused: true,
            warnings: Vec::new(),
        });
    }

    // Resolve to the first free (path, branch) pair, suffixing -2, -3, … .
    let mut workspace = base_workspace.clone();
    let mut branch = base_branch.clone();
    let mut work_dir = base_dir.clone();
    let mut session_label = format!("{prefix}{short_label}");
    let mut n = 2;
    while work_dir.is_dir() || local_branch_exists(&cloned_repo, &branch).await {
        workspace = format!("{base_workspace}-{n}");
        branch = format!("{base_branch}-{n}");
        work_dir = worktree_dir(&workspace);
        session_label = format!("{prefix}{short_label} ({n})");
        n += 1;
    }
    // Re-record once the final (possibly suffixed) workspace id is resolved.
    tracing::Span::current().record("session", workspace.as_str());

    let session_title = format!("{emoji} {session_label}");
    let work_parent = work_dir.parent().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();

    // Record the session up front so the dashboard shows the row immediately
    // (and so its color counts as taken for the next spawn) — the worktree it
    // points at is built by the background task below. If we can't even record
    // it, fail synchronously: the row would never appear.
    let session = crate::sessions::Session {
        id: workspace.clone(),
        repo: repo.to_string(),
        issue_number,
        issue_url: issue_url.to_string(),
        branch: branch.clone(),
        default_branch: default_branch.to_string(),
        work_dir: work_dir.display().to_string(),
        cloned_repo_dir: cloned_repo.display().to_string(),
        session_title: session_title.clone(),
        color: color.to_string(),
        emoji: emoji.to_string(),
        hidden: None,
    };
    crate::sessions::save(&session).map_err(|e| format!("could not record session: {e}"))?;

    // Mark it `creating` so the popover shows a "Creating…" pill while the
    // background task builds the worktree.
    crate::status::write_creating(&workspace);

    // Hand the slow work off to a background task and return at once.
    let bg = SpawnBg {
        gh,
        cloned_repo,
        work_dir: work_dir.clone(),
        work_parent,
        branch: branch.clone(),
        workspace: workspace.clone(),
        session_title,
        color: color.to_string(),
        default_branch: default_branch.to_string(),
        repo: repo.to_string(),
        issue_number,
        env_files: settings.env_files.clone(),
        post_spawn_commands: settings.post_spawn_commands.clone(),
    };
    tokio::spawn(finish_spawn(bg));

    tracing::info!(repo = %repo, issue = issue_number, branch = %branch, reused = false, "spawn started (building worktree in background)");
    Ok(SpawnResult {
        session_id: workspace,
        work_dir: work_dir.display().to_string(),
        branch,
        issue_url: issue_url.to_string(),
        reused: false,
        warnings: Vec::new(),
    })
}

/// Background phase of a fresh spawn: build the worktree and launch the editor
/// after `do_spawn` has already returned. On success the `creating` marker is
/// cleared (handing the status over to Claude's hooks); on a fatal failure it's
/// replaced with a surfaced error the popover shows on the row. Carries the
/// `session=` span so its log lines join the rest of the spawn's story.
async fn finish_spawn(bg: SpawnBg) {
    use tracing::Instrument;
    let span = tracing::info_span!("finish_spawn", session = %bg.workspace);
    async move {
        match do_finish_spawn(&bg).await {
            Ok(warnings) => {
                for w in &warnings {
                    tracing::warn!(warning = %w, "spawn warning");
                }
                tracing::info!(branch = %bg.branch, "spawn finished");
            }
            Err(e) => {
                tracing::error!(error = %e, "spawn failed");
                crate::status::write_spawn_error(&bg.workspace, &e);
            }
        }
    }
    .instrument(span)
    .await;
}

/// True if `rel` is a safe *relative* path to copy inside the cloned repo/worktree:
/// non-empty, not absolute, and with no `..` component — so `cloned_repo.join(rel)`
/// and `work_dir.join(rel)` cannot escape their base dirs. An `env_files` entry is
/// user-authored (repo settings) and normally a bare name like `.env.local`, but an
/// absolute (`/Users/me/.ssh/id_rsa`) or `..`-laden entry would otherwise copy an
/// arbitrary file *into* the worktree, or write the copy *outside* it.
fn is_contained_relpath(rel: &str) -> bool {
    use std::path::{Component, Path};
    !rel.is_empty()
        && Path::new(rel)
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// The actual worktree build, factored out so `finish_spawn` can map its result
/// to the status record. Returns non-fatal warnings on success; an `Err` is a
/// fatal failure (e.g. `git worktree add`) that leaves no usable worktree.
async fn do_finish_spawn(bg: &SpawnBg) -> Result<Vec<String>, String> {
    let mut warnings = Vec::new();

    // Create the worktree from the repo's default branch.
    std::fs::create_dir_all(bg.work_dir.parent().unwrap()).map_err(|e| e.to_string())?;
    if let Err(e) = git_net(&bg.cloned_repo, &["fetch", "origin", "--quiet"]).await {
        tracing::warn!(error = %e, "git fetch before spawn failed (continuing)");
    }
    git(&bg.cloned_repo, &["worktree", "add", &bg.work_dir.to_string_lossy(), "-b", &bg.branch, &format!("origin/{}", bg.default_branch)]).await?;
    if let Err(e) = git(&bg.work_dir, &["branch", "--unset-upstream"]).await {
        tracing::warn!(error = %e, "git branch --unset-upstream failed (continuing)");
    }

    // Copy configured env files (relative to the cloned repo) into the worktree.
    for rel in &bg.env_files {
        // Reject absolute or `..`-escaping entries so the copy can't read outside
        // the cloned repo or write outside the worktree.
        if !is_contained_relpath(rel) {
            warnings.push(format!("env file path not contained in the cloned repo, skipped: {rel}"));
            continue;
        }
        let src = bg.cloned_repo.join(rel);
        if !src.is_file() {
            warnings.push(format!("env file not found, skipped: {rel}"));
            continue;
        }
        let dst = bg.work_dir.join(rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        if let Err(e) = std::fs::copy(&src, &dst) {
            warnings.push(format!("could not copy env file {rel}: {e}"));
        }
    }

    // Assign the issue to the token's user and record the workspace in a
    // comment. Non-fatal: the worktree already exists, so failures only warn.
    match bg.gh.authenticated_login().await {
        Ok(login) => {
            if let Err(e) = bg.gh.add_assignees(&bg.repo, bg.issue_number, &[login]).await {
                warnings.push(format!("could not assign issue #{}: {e}", bg.issue_number));
            }
            let body = format!(
                "🤖 Spawned a local workspace for this issue.\n\n\
                 - **GitHub Branch:** `{}`\n\
                 - **Local Directory:** `{}`\n\
                 - **Claude Session:** `{}`\n",
                bg.branch,
                bg.work_dir.display(),
                bg.session_title,
            );
            if let Err(e) = bg.gh.create_comment(&bg.repo, bg.issue_number, &body).await {
                warnings.push(format!("could not comment on issue #{}: {e}", bg.issue_number));
            }
        }
        Err(e) => warnings.push(format!("could not resolve token user for assignment: {e}")),
    }

    write_vscode_files(&bg.work_dir, &bg.work_parent, &bg.color, &bg.session_title)?;
    write_claude_hooks(&bg.work_dir, &bg.workspace).await?;

    // Run the repo's post-spawn commands (e.g. `pnpm install`) in the new
    // worktree before opening the editor, so the session starts ready.
    warnings.extend(run_post_spawn_commands(&bg.work_dir, &bg.post_spawn_commands).await);

    // Worktree is ready: clear the `creating` marker before opening the editor,
    // so Claude's SessionStart hook (fired only once VS Code launches it) owns
    // the status from here without us racing to clobber it.
    crate::status::clear_creating(&bg.workspace);

    open_vscode(&bg.work_dir)?;
    Ok(warnings)
}

/// Fetch an issue's facts (title, url, body) and — via the caller — the repo's
/// default branch: the shared first step of preparing or running a spawn for an
/// existing issue. Also used by the drafting path's short-label suggestion.
pub(crate) async fn issue_facts(gh: &GitHub, repo: &str, issue_number: u64) -> Result<(String, String, String), String> {
    let issue = gh.issue(repo, issue_number).await?;
    let issue_title = issue["title"].as_str().unwrap_or("").to_string();
    if issue_title.is_empty() {
        return Err(format!("issue #{issue_number} not found"));
    }
    let issue_url = issue["html_url"].as_str().unwrap_or("").to_string();
    let issue_body = issue["body"].as_str().unwrap_or("").to_string();
    Ok((issue_title, issue_url, issue_body))
}

/// Spawn directly from an existing issue using default theming and a heuristic
/// short label (no preview). Kept for completeness/back-compat; the UI now goes
/// through `prepare_spawn` + `confirm_spawn`.
#[tauri::command]
pub async fn spawn_work(repo: String, issue_number: u64, force_new: bool) -> Result<SpawnResult, String> {
    crate::log_invoke!("spawn_work", repo = %repo, issue = issue_number, force_new);
    let (_settings, gh) = repo_context(&repo).await?;
    let (issue_title, issue_url, _) = issue_facts(&gh, &repo, issue_number).await?;
    let default_branch = gh.repo(&repo).await?["default_branch"].as_str().unwrap_or("main").to_string();
    let short_label = default_short_title(&issue_title);
    let seed = format!("{issue_number}-{}", slugify(&short_label, 25));
    let (color, emoji) = pick_theme(&seed);
    do_spawn(SpawnDecision {
        repo: &repo,
        issue_number,
        issue_url: &issue_url,
        default_branch: &default_branch,
        short_label: &short_label,
        color,
        emoji,
        force_new,
    })
    .await
}

/// The effective worktree-path prefix: the configured value, or the schema
/// default (`/properties/worktree_prefix/default`) when unset or empty. The
/// default lives in the JSON schema only — no hardcoded fallback here. Takes the
/// field rather than the whole settings so callers that have already moved other
/// fields out can still use it.
fn effective_worktree_prefix(configured: Option<&str>) -> String {
    configured
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| crate::repo_settings::schema_default("/properties/worktree_prefix/default"))
}

/// Run a repo's post-spawn commands in the freshly-created worktree, in order.
/// Each runs via the user's login shell (`$SHELL -lc`) so PATH and tool managers
/// (nvm, pnpm, asdf, …) are available — mAIestro's own environment is minimal and
/// not sourced from a profile. Stops at the first command that fails or times
/// out; returns a warning per problem (the worktree is left in place either way,
/// never torn down). A blank command is skipped.
async fn run_post_spawn_commands(work_dir: &Path, commands: &[String]) -> Vec<String> {
    let mut warnings = Vec::new();
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    for cmd in commands {
        let cmd = cmd.trim();
        if cmd.is_empty() {
            continue;
        }
        tracing::info!(command = %cmd, "running post-spawn command");
        let run = tokio::process::Command::new(&shell)
            .args(["-l", "-c", cmd])
            .current_dir(work_dir)
            // Without this, a command that hits the timeout below is orphaned
            // and keeps mutating the worktree under the live session.
            .kill_on_drop(true)
            .output();
        // 10 minutes is generous for installs but still bounds a hung command so
        // it can't freeze the spawn forever.
        let output = tokio::time::timeout(std::time::Duration::from_secs(600), run).await;
        match output {
            Ok(Ok(out)) if out.status.success() => {
                tracing::info!(command = %cmd, "post-spawn command succeeded");
            }
            Ok(Ok(out)) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let code = out.status.code().unwrap_or(-1);
                tracing::error!(command = %cmd, code, stderr = %snippet(&stderr), "post-spawn command failed");
                warnings.push(format!("post-spawn command failed (exit {code}): {cmd}"));
                break;
            }
            Ok(Err(e)) => {
                tracing::error!(command = %cmd, error = %e, "could not run post-spawn command");
                warnings.push(format!("could not run post-spawn command '{cmd}': {e}"));
                break;
            }
            Err(_) => {
                tracing::error!(command = %cmd, "post-spawn command timed out");
                warnings.push(format!("post-spawn command timed out after 10m: {cmd}"));
                break;
            }
        }
    }
    warnings
}

// ── Create-issue-and-spawn ──────────────────────────────────────────────────────

/// Draft an issue from the user's idea (via Claude) and open it on GitHub,
/// **without** spawning a workspace.
///
/// When `use_raw_fallback` is false and Claude can't produce a clear draft
/// (e.g. the idea is too vague and it asks for clarification), this creates
/// nothing and returns `NeedsConfirmation` carrying Claude's reply, so the UI
/// can ask the user whether to proceed. Calling again with `use_raw_fallback`
/// true skips drafting and creates the issue straight from the user's text.
#[tauri::command]
pub async fn create_issue(
    app: tauri::AppHandle,
    repo: String,
    idea: String,
    use_raw_fallback: bool,
    request_id: String,
) -> Result<CreateIssueOutcome, String> {
    crate::log_invoke!("create_issue", repo = %repo, use_raw_fallback);
    let activity = ClaudeActivity::new(app, request_id);
    let (gh, step) = resolve_draft(&repo, &idea, use_raw_fallback, &activity).await?;
    let (title, body, warning) = match step {
        DraftStep::Ready { title, body, warning, .. } => (title, body, warning),
        DraftStep::NeedsConfirmation { message } => {
            return Ok(CreateIssueOutcome::NeedsConfirmation { message });
        }
    };

    let number = gh.create_issue(&repo, &title, &body).await?;
    Ok(CreateIssueOutcome::Created {
        number,
        // The create endpoint only returns the number; the html_url is derivable
        // (the whole app assumes github.com — see plugins/github.rs).
        issue_url: format!("https://github.com/{repo}/issues/{number}"),
        warnings: warning.into_iter().collect(),
    })
}

/// Open an issue from an explicit, already-reviewed title and body (no drafting).
/// Used by the create-issue preview's confirm button.
#[tauri::command]
pub async fn create_issue_direct(repo: String, title: String, body: String) -> Result<CreateIssueOutcome, String> {
    crate::log_invoke!("create_issue_direct", repo = %repo);
    let title = title.trim();
    if title.is_empty() {
        return Err("Issue title can't be empty.".into());
    }
    let (_settings, gh) = repo_context(&repo).await?;
    let number = gh.create_issue(&repo, title, &body).await?;
    Ok(CreateIssueOutcome::Created {
        number,
        issue_url: format!("https://github.com/{repo}/issues/{number}"),
        warnings: Vec::new(),
    })
}

/// Draft an issue from the user's idea (via Claude), open it on GitHub, then
/// spawn a workspace for the freshly created issue. See `create_issue` for the
/// drafting / needs-confirmation semantics.
#[tauri::command]
pub async fn create_issue_and_spawn(
    app: tauri::AppHandle,
    repo: String,
    idea: String,
    use_raw_fallback: bool,
    force_new: bool,
    request_id: String,
) -> Result<CreateAndSpawnOutcome, String> {
    crate::log_invoke!("create_issue_and_spawn", repo = %repo, use_raw_fallback, force_new);
    let activity = ClaudeActivity::new(app, request_id);
    let (gh, step) = resolve_draft(&repo, &idea, use_raw_fallback, &activity).await?;
    let (title, body, draft_warning) = match step {
        DraftStep::Ready { title, body, warning, .. } => (title, body, warning),
        DraftStep::NeedsConfirmation { message } => {
            return Ok(CreateAndSpawnOutcome::NeedsConfirmation { message });
        }
    };

    let number = gh.create_issue(&repo, &title, &body).await?;
    let mut result = spawn_work(repo, number, force_new).await?;
    if let Some(w) = draft_warning {
        result.warnings.insert(0, w);
    }
    Ok(CreateAndSpawnOutcome::Spawned(result))
}

// ── Preview-then-spawn ────────────────────────────────────────────────────────

/// Everything the spawn preview shows for an issue: the editable issue fields
/// and short label, plus the chosen theming and the repo's local dir name (so
/// the UI can render the worktree path). `issue_number` is None on the
/// create-and-spawn path, where the issue isn't opened until the user confirms.
#[derive(serde::Serialize)]
pub struct SpawnPlan {
    pub repo: String,
    pub issue_number: Option<u64>,
    pub issue_title: String,
    pub issue_body: String,
    pub short_title: String,
    pub color: String,
    pub emoji: String,
    pub repo_name: String,
    /// Effective (un-expanded) worktree prefix, so the preview can show where
    /// the worktree will actually land instead of hardcoding the default.
    pub worktree_prefix: String,
}

/// Prepare a preview for spawning an existing issue: fetch its title/body and
/// pick theming, without touching the worktree or GitHub.
#[tauri::command]
pub async fn prepare_spawn(repo: String, issue_number: u64) -> Result<SpawnPlan, String> {
    crate::log_invoke!("prepare_spawn", repo = %repo, issue = issue_number);
    let (settings, gh) = repo_context(&repo).await?;
    let worktree_prefix = effective_worktree_prefix(settings.worktree_prefix.as_deref());
    let (issue_title, _issue_url, issue_body) = issue_facts(&gh, &repo, issue_number).await?;
    let short_title = default_short_title(&issue_title);
    let seed = format!("{issue_number}-{}", slugify(&short_title, 25));
    let (color, emoji) = pick_theme(&seed);
    let repo_name = repo.split('/').next_back().unwrap_or(&repo).to_string();
    Ok(SpawnPlan {
        repo,
        issue_number: Some(issue_number),
        issue_title,
        issue_body,
        short_title,
        color: color.to_string(),
        emoji: emoji.to_string(),
        repo_name,
        worktree_prefix,
    })
}

/// Result of drafting a spawn preview from a free-text idea: a ready preview, or
/// a needs-confirmation prompt (Claude couldn't draft a clear issue).
#[derive(serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DraftPreviewOutcome {
    Drafted(SpawnPlan),
    NeedsConfirmation { message: String },
}

/// Draft an issue from the user's idea (one Claude call, which also yields the
/// short label) and return a preview — WITHOUT creating the issue. The issue is
/// only opened when the user confirms via `confirm_spawn`.
#[tauri::command]
pub async fn draft_spawn_preview(
    app: tauri::AppHandle,
    repo: String,
    idea: String,
    use_raw_fallback: bool,
    request_id: String,
) -> Result<DraftPreviewOutcome, String> {
    crate::log_invoke!("draft_spawn_preview", repo = %repo, use_raw_fallback);
    let activity = ClaudeActivity::new(app, request_id);
    let (_gh, step) = resolve_draft(&repo, &idea, use_raw_fallback, &activity).await?;
    match step {
        DraftStep::Ready { title, body, short_title, .. } => {
            let seed = format!("new-{}", slugify(&short_title, 25));
            let (color, emoji) = pick_theme(&seed);
            let repo_name = repo.split('/').next_back().unwrap_or(&repo).to_string();
            let settings = crate::repo_settings::repo_settings_get(repo.clone())?;
            let worktree_prefix = effective_worktree_prefix(settings.worktree_prefix.as_deref());
            Ok(DraftPreviewOutcome::Drafted(SpawnPlan {
                repo,
                issue_number: None,
                issue_title: title,
                issue_body: body,
                short_title,
                color: color.to_string(),
                emoji: emoji.to_string(),
                repo_name,
                worktree_prefix,
            }))
        }
        DraftStep::NeedsConfirmation { message } => Ok(DraftPreviewOutcome::NeedsConfirmation { message }),
    }
}

/// The reviewed (possibly edited) preview the user confirmed. `issue_number` is
/// Some for an existing issue (PATCHed when `update_issue`), None to create one.
#[derive(serde::Deserialize)]
pub struct SpawnEdits {
    pub issue_number: Option<u64>,
    pub issue_title: String,
    pub issue_body: String,
    pub short_title: String,
    pub color: String,
    pub emoji: String,
    /// For an existing issue: whether the title/body were changed and should be
    /// written back to GitHub. Ignored on the create path (always created).
    pub update_issue: bool,
}

/// Confirm a previewed spawn: create or update the GitHub issue as needed, then
/// build the worktree/session using the reviewed label and theming.
#[tauri::command]
pub async fn confirm_spawn(repo: String, edits: SpawnEdits, force_new: bool) -> Result<SpawnResult, String> {
    crate::log_invoke!("confirm_spawn", repo = %repo, force_new);
    let (_settings, gh) = repo_context(&repo).await?;

    let number = match edits.issue_number {
        Some(n) => {
            if edits.update_issue {
                gh.update_issue(&repo, n, &edits.issue_title, &edits.issue_body).await?;
            }
            n
        }
        None => gh.create_issue(&repo, &edits.issue_title, &edits.issue_body).await?,
    };

    let default_branch = gh.repo(&repo).await?["default_branch"].as_str().unwrap_or("main").to_string();
    let issue_url = format!("https://github.com/{repo}/issues/{number}");

    do_spawn(SpawnDecision {
        repo: &repo,
        issue_number: number,
        issue_url: &issue_url,
        default_branch: &default_branch,
        short_label: &edits.short_title,
        color: &edits.color,
        emoji: &edits.emoji,
        force_new,
    })
    .await
}

// ── Teardown ────────────────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TeardownOutcome {
    Done,
    /// Checks found unresolved work; `warnings` describes it so the UI can ask
    /// the user to confirm before destroying the worktree.
    NeedsConfirmation { warnings: Vec<String> },
    /// VS Code still has the worktree open and we couldn't close it (no
    /// Accessibility grant, or the close didn't take). `message` explains the
    /// situation; `accessibility` is true when granting Accessibility would let
    /// mAIestro close the window itself, so the UI can offer that shortcut.
    BlockedByEditor { message: String, accessibility: bool },
}

/// Tear down a spawned session's worktree. Inspects the branch first
/// (uncommitted changes, PR state, unmerged commits); confirmation is required
/// in every case except when the PR is merged and nothing new remains. On
/// teardown the VS Code window is closed *first* (open windows have caused
/// removal failures), then the worktree, local branch, directory, and session
/// record are removed.
#[tauri::command]
#[tracing::instrument(skip_all, fields(session = %session_id))]
pub async fn teardown(session_id: String, confirmed: bool, force: bool) -> Result<TeardownOutcome, String> {
    crate::log_invoke!("teardown", confirmed, force);
    let session = crate::sessions::get(&session_id)
        .ok_or_else(|| format!("session not found: {session_id}"))?;
    let work_dir = PathBuf::from(&session.work_dir);
    let cloned_repo = expand_tilde(&session.cloned_repo_dir);
    let branch = session.branch.clone();
    let base = session.default_branch.clone();

    // ── Checks ──────────────────────────────────────────────────────────────
    let mut warnings = Vec::new();

    let dirty = git(&work_dir, &["status", "--porcelain"])
        .await
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    if dirty {
        warnings.push("Worktree has uncommitted changes".to_string());
    }

    // PR state via the REST API (best-effort: needs an identity + token).
    let mut pr_merged = false;
    let settings = crate::repo_settings::repo_settings_get(session.repo.clone())?;
    if let Some(identity_id) = settings.identity_id {
        if let Ok(gh) = GitHub::for_identity(&identity_id).await {
            if let Ok(prs) = gh.pulls_for_branch(&session.repo, &branch).await {
                pr_merged = prs.iter().any(|p| p["merged_at"].is_string());
                if let Some(open) = prs.iter().find(|p| p["state"].as_str() == Some("open")) {
                    warnings.push(format!("PR #{} is still open", open["number"].as_u64().unwrap_or(0)));
                }
            }
            // Fallback: once a PR merges, GitHub deletes its head branch by
            // default, after which the head-ref filter above returns nothing and
            // we'd wrongly conclude "no work on this branch". Resolve by the
            // branch's tip commit instead, which still points at the merged PR.
            if !pr_merged {
                if let Ok(sha) = git(&work_dir, &["rev-parse", "HEAD"]).await {
                    if let Ok(prs) = gh.pulls_for_commit(&session.repo, &sha).await {
                        pr_merged = prs.iter().any(|p| p["merged_at"].is_string());
                    }
                }
            }
        }
    }

    // Commits on the branch not yet on the base, when no merged PR accounts for them.
    let ahead = git(&work_dir, &["rev-list", "--count", &format!("origin/{base}..{branch}")])
        .await
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);
    if !pr_merged && ahead > 0 {
        warnings.push(format!("Branch has {ahead} commit(s) not merged"));
    }
    if !pr_merged && ahead == 0 && !dirty {
        warnings.push("Nothing has been done on this branch".to_string());
    }

    // Skip confirmation only when the PR is merged and nothing new remains.
    let safe = pr_merged && !dirty;
    if !safe && !confirmed {
        return Ok(TeardownOutcome::NeedsConfirmation { warnings });
    }

    // ── Execute ─────────────────────────────────────────────────────────────
    // 1. Close VS Code FIRST and CONFIRM the worktree is free before touching the
    //    files. Removing it out from under a live VS Code crashes the editor, so
    //    we only proceed once we can show the window is gone — never on a guess.
    //    `force` skips this entirely: the user chose "Delete anyway" knowing the
    //    open window may crash.
    if !force {
        if let Some(marker) = window_marker(&work_dir) {
            close_editor_window(&marker).await;
            let mut waited = 0u64;
            loop {
                match probe_editor_window(&marker).await {
                    // Window confirmed gone — safe to delete.
                    WinProbe::Absent => break,
                    // No Accessibility grant: we can neither close nor see the
                    // window. Fall back to the permission-free check — if nothing
                    // is using the worktree, proceed; otherwise stop and let the
                    // user close the window, grant Accessibility, or force it.
                    WinProbe::Denied => {
                        if worktree_in_use(&work_dir).await {
                            return Ok(TeardownOutcome::BlockedByEditor {
                                message:
                                    "I couldn't tear down because the Visual Studio Code window \
                                     is still open.\n\nYou have two options: close the window \
                                     yourself, or enable Accessibility for mAIestro so it can \
                                     close the window for you."
                                        .to_string(),
                                accessibility: true,
                            });
                        }
                        break;
                    }
                    // Window still open with Accessibility granted: the close is in
                    // flight (or the user may close it). Wait a bit, then give up.
                    WinProbe::Open => {
                        if waited >= 4000 {
                            return Ok(TeardownOutcome::BlockedByEditor {
                                message:
                                    "I couldn't tear down because the Visual Studio Code window \
                                     is still open. Close its window, then try Tear Down again."
                                        .to_string(),
                                accessibility: false,
                            });
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                        waited += 300;
                    }
                }
            }
        }
    }

    // 2. Remove the worktree (force: the user confirmed discarding any changes).
    //    A spawn that failed in the background (issue #77) can leave a Session
    //    record whose worktree was never created — tolerate a missing dir so the
    //    broken row can still be torn down, just pruning any dangling admin entry.
    if work_dir.exists() {
        git(&cloned_repo, &["worktree", "remove", "--force", &work_dir.to_string_lossy()]).await?;
    } else {
        let _ = git(&cloned_repo, &["worktree", "prune"]).await;
    }

    // 3. Delete the local branch (-D: spawn unset the upstream and -d checks the
    //    wrong base, so it would refuse even for merged branches).
    if local_branch_exists(&cloned_repo, &branch).await {
        if let Err(e) = git(&cloned_repo, &["branch", "-D", &branch]).await {
            tracing::warn!(branch = %branch, error = %e, "could not delete local branch during teardown");
        }
    }

    // 4. Remove the leftover wrapper dir (`<prefix><workspace>`), gating removal
    //    on the path actually starting with the repo's configured worktree
    //    prefix so we never remove_dir_all something outside it. A spawn-generated
    //    path never contains `..`; reject any that does before the string-prefix
    //    check, so a tampered session record can't tunnel out of the prefix (e.g.
    //    `.../work-x/../../../etc`) while still matching the prefix literally.
    if let Some(parent) = work_dir.parent() {
        let prefix = effective_worktree_prefix(settings.worktree_prefix.as_deref());
        let expanded = expand_tilde(&prefix);
        let has_dotdot = parent
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir));
        let under_prefix = parent.to_string_lossy().starts_with(&*expanded.to_string_lossy());
        // Only remove the wrapper once it's empty: the wrapper is keyed by
        // `<issue>-<slug>` alone, so two repos with an identically-slugged issue
        // share it — removing it while the sibling's worktree is inside would
        // destroy that repo's work.
        let empty = std::fs::read_dir(parent).map(|mut d| d.next().is_none()).unwrap_or(false);
        if !has_dotdot && under_prefix && parent.exists() {
            if !empty {
                tracing::info!(dir = %parent.display(), "wrapper dir not empty after teardown; leaving it in place");
            } else if let Err(e) = std::fs::remove_dir_all(parent) {
                tracing::warn!(dir = %parent.display(), error = %e, "could not remove worktree wrapper dir during teardown");
            }
        }
    }

    // 5. Drop the session record and its live status file.
    if let Err(e) = crate::sessions::delete(&session_id) {
        tracing::warn!(error = %e, "could not delete session record during teardown");
    }
    crate::status::remove(&session_id);

    tracing::info!(branch = %branch, "tore down workspace");
    Ok(TeardownOutcome::Done)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bare name and a nested relative path are contained; empty, absolute,
    /// and `..`-escaping entries are rejected so the env-file copy can't read
    /// outside the cloned repo or write outside the worktree.
    #[test]
    fn is_contained_relpath_accepts_relative_rejects_escapes() {
        assert!(is_contained_relpath(".env"));
        assert!(is_contained_relpath("frontend/.env.local"));
        assert!(is_contained_relpath("./config/.env"));
        assert!(!is_contained_relpath(""));
        assert!(!is_contained_relpath("/etc/passwd"));
        assert!(!is_contained_relpath("../secrets/.env"));
        assert!(!is_contained_relpath("a/../../b"));
    }
}
