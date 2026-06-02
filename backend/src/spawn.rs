//! spawn_work — create a git worktree + VS Code workspace for a GitHub issue.
//!
//! GitHub is reached via the REST API under the repo's identity (no `gh`), env
//! files come from the repo's settings, and the editor is launched via `open -a`
//! (no constructed env).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::plugins::GitHub;

// ── Worktree title-bar theming ──────────────────────────────────────────────

const PALETTE: &[&str] = &[
    "#1a3a6c", "#4a1a6c", "#1a6c5a", "#6c1a1a",
    "#6c3a1a", "#1a6c2a", "#6c1a5a", "#1a5a6c",
];

/// Emoji options per palette color — one is chosen (deterministically) per spawn.
fn palette_emojis(color: &str) -> &'static [&'static str] {
    match color {
        "#1a3a6c" => &["🔵", "🌊", "🫐", "🦋", "🧊", "💙"], // navy blue
        "#4a1a6c" => &["🟣", "🔮", "💜"],                    // dark purple
        "#1a6c5a" => &["🐢", "🌴", "🐠", "🍃", "🦚"],        // dark teal
        "#6c1a1a" => &["🔴", "🍒", "🌶️", "🦞"],             // dark red
        "#6c3a1a" => &["🟠", "🦊", "🍊", "🦁"],              // dark orange
        "#1a6c2a" => &["🌿", "🐸", "🍀", "🐊"],              // dark green
        "#6c1a5a" => &["🔮", "🎀"],                          // dark magenta
        "#1a5a6c" => &["🩵", "🐬", "🧊", "🐟"],              // dark cyan
        _ => &["🔵"],
    }
}

// ── Small helpers ─────────────────────────────────────────────────────────────

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

/// Expand a leading `~` to $HOME. Other paths pass through unchanged.
fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest)
    } else if p == "~" {
        home()
    } else {
        PathBuf::from(p)
    }
}

/// Deterministic index into a list of `len`, derived from `s` and a `salt`.
/// DefaultHasher::new() is fixed-seeded, so the same name always themes the same.
fn hash_index(s: &str, salt: u64, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let mut h = DefaultHasher::new();
    salt.hash(&mut h);
    s.hash(&mut h);
    (h.finish() % len as u64) as usize
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn local_branch_exists(checkout: &Path, name: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(["rev-parse", "--verify", "--quiet", &format!("refs/heads/{name}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Trim `s` to at most `max` chars, ending at a word boundary when one fits.
fn trim_to_word(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    match cut.rfind(' ') {
        Some(i) if i > 0 => cut[..i].trim_end().to_string(),
        _ => cut.trim_end().to_string(),
    }
}

fn slugify(title: &str, max_len: usize) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;
    for c in title.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.len() <= max_len {
        return slug;
    }
    let cut = &slug[..max_len];
    match cut.rfind('-') {
        Some(i) if i > 0 => cut[..i].trim_end_matches('-').to_string(),
        _ => cut.trim_end_matches('-').to_string(),
    }
}

// ── VS Code workspace files ─────────────────────────────────────────────────────

/// Single-quote a string for safe inclusion in a POSIX shell command (the
/// task's `command` runs through a shell). Wraps in single quotes and escapes
/// any embedded single quote as `'\''`.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn write_vscode_files(work_dir: &Path, work_parent: &str, color: &str, session_title: &str) -> Result<(), String> {
    let vscode = work_dir.join(".vscode");
    std::fs::create_dir_all(&vscode).map_err(|e| e.to_string())?;

    let settings = serde_json::json!({
        "workbench.colorCustomizations": {
            "titleBar.activeBackground": color,
            "titleBar.inactiveBackground": format!("{color}99"),
            "statusBar.background": color,
            "activityBar.background": color,
        },
        "task.allowAutomaticTasks": "on",
        "workbench.startupEditor": "none",
        "workbench.secondarySideBar.visible": false,
        // Let teardown close the window without a "Are you sure?" prompt blocking
        // the programmatic close (dirty files are preserved via hot exit).
        "window.confirmBeforeClose": "never",
        // Marker used by teardown to find this window via AppleScript.
        "window.title": format!("${{dirty}}${{activeEditorShort}}${{separator}}{work_parent}/${{rootName}}"),
        "terminal.integrated.gpuAcceleration": "off",
    });
    std::fs::write(
        vscode.join("settings.json"),
        serde_json::to_string_pretty(&settings).unwrap() + "\n",
    )
    .map_err(|e| e.to_string())?;

    // Folder-open task that starts a real, user-facing Claude session in the
    // integrated terminal. --remote-control lets the user drive the session
    // remotely; mAIestro still only launches it, it does not host it. --name
    // gives the session the same display name mAIestro tracks it by.
    let command = format!(
        "claude --remote-control --name {}",
        shell_quote(session_title)
    );
    let tasks = serde_json::json!({
        "version": "2.0.0",
        "tasks": [{
            "label": "Start Claude",
            "type": "shell",
            "command": command,
            "isBackground": true,
            "problemMatcher": [],
            "presentation": { "reveal": "always", "panel": "new", "focus": true },
            "runOptions": { "runOn": "folderOpen" },
        }],
    });
    std::fs::write(
        vscode.join("tasks.json"),
        serde_json::to_string_pretty(&tasks).unwrap() + "\n",
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Locate the VS Code `code` CLI: $PATH first, then common install locations
/// (the bundled CLI inside the .app is the most reliable when $PATH is minimal).
fn code_cli() -> Option<PathBuf> {
    which("code").or_else(|| {
        [
            "/opt/homebrew/bin/code",
            "/usr/local/bin/code",
            "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code",
        ]
        .into_iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
    })
}

fn open_vscode(dir: &Path) -> Result<(), String> {
    // Prefer the `code` CLI so we can pass --disable-workspace-trust and skip the
    // "Do you trust the authors of the files in this folder?" prompt on every
    // freshly spawned worktree. The CLI forwards the flag even to an already
    // running VS Code, which `open -a --args` cannot.
    if let Some(code) = code_cli() {
        Command::new(code)
            .arg("--disable-workspace-trust")
            .arg(dir)
            .spawn()
            .map_err(|e| format!("failed to open VS Code: {e}"))?;
        return Ok(());
    }
    // Fallback: Launch Services. --args forwards the flag, but only honored when
    // VS Code isn't already running.
    Command::new("open")
        .args(["-a", "Visual Studio Code", "--args", "--disable-workspace-trust"])
        .arg(dir)
        .spawn()
        .map_err(|e| format!("failed to open VS Code: {e}"))?;
    Ok(())
}

/// The substring that identifies a worktree's VS Code window — the same
/// `work_parent/rootName` we bake into `window.title` on spawn.
fn window_marker(work_dir: &Path) -> Option<String> {
    let name = work_dir.file_name()?.to_str()?;
    let parent = work_dir.parent()?.file_name()?.to_str()?;
    Some(format!("{parent}/{name}"))
}

/// Look for an open VS Code window whose title contains `marker` and, if found,
/// raise it to the front and activate the app. Returns true when one was
/// focused. Requires Accessibility permission for System Events; any failure
/// (including a missing grant) is treated as "not found" so the caller can fall
/// back to launching a window.
async fn focus_editor_window(marker: &str) -> bool {
    // marker is path-safe (slug + repo dir name); strip quotes defensively.
    let safe = marker.replace('"', "");
    let script = format!(
        r#"tell application "System Events"
  if not (exists process "Code") then return "notfound"
  tell process "Code"
    repeat with w in windows
      if name of w contains "{safe}" then
        perform action "AXRaise" of w
        set frontmost to true
        return "focused"
      end if
    end repeat
  end tell
end tell
return "notfound""#
    );
    match tokio::process::Command::new("osascript").arg("-e").arg(&script).output().await {
        Ok(out) => String::from_utf8_lossy(&out.stdout).trim() == "focused",
        Err(_) => false,
    }
}

/// Open the worktree in VS Code: focus (and bring to the front) an existing
/// window for that folder if one is open, otherwise launch a new window.
#[tauri::command]
pub async fn open_in_editor(work_dir: String) -> Result<(), String> {
    let path = PathBuf::from(&work_dir);
    if let Some(marker) = window_marker(&path) {
        if focus_editor_window(&marker).await {
            return Ok(());
        }
    }
    open_vscode(&path)
}

// ── Command ─────────────────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct SpawnResult {
    pub work_dir: String,
    pub branch: String,
    pub issue_url: String,
    /// True when an existing workspace was reused rather than created.
    pub reused: bool,
    /// Non-fatal warnings (e.g. GitHub assign/comment failures, missing env files).
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

#[tauri::command]
pub async fn spawn_work(repo: String, issue_number: u64, force_new: bool) -> Result<SpawnResult, String> {
    let settings = crate::repo_settings::repo_settings_get(repo.clone());
    let identity_id = settings
        .identity_id
        .clone()
        .ok_or_else(|| "No identity assigned to this repo. Set one in Settings → Repo.".to_string())?;

    let checkout = expand_tilde(settings.checkout_dir.as_deref().unwrap_or_default());
    if !checkout.join(".git").exists() {
        return Err(format!("checkout dir is not a git repo: {}", checkout.display()));
    }
    let repo_name = repo.split('/').next_back().unwrap_or(&repo).to_string();

    let gh = GitHub::for_identity(&identity_id)?;
    let issue = gh.issue(&repo, issue_number).await?;
    let issue_title = issue["title"].as_str().unwrap_or("").to_string();
    let issue_url = issue["html_url"].as_str().unwrap_or("").to_string();
    if issue_title.is_empty() {
        return Err(format!("issue #{issue_number} not found"));
    }
    let default_branch = gh.repo(&repo).await?["default_branch"].as_str().unwrap_or("main").to_string();

    // Workspace name: "<n>-<slug>"; session title budgets the label to ~30 chars.
    let prefix = format!("#{issue_number} — ");
    let label_budget = 30usize.saturating_sub(prefix.chars().count());
    let short_label = trim_to_word(&issue_title, label_budget);
    let base_workspace = format!("{issue_number}-{}", slugify(&short_label, 25));
    let base_branch = format!("feature/{base_workspace}");
    let base_dir = home().join(format!("src/work-{base_workspace}")).join(&repo_name);

    // Reuse an existing workspace by default; --new forces a fresh one.
    if !force_new && base_dir.is_dir() {
        open_vscode(&base_dir)?;
        return Ok(SpawnResult {
            work_dir: base_dir.display().to_string(),
            branch: base_branch,
            issue_url,
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
    while work_dir.is_dir() || local_branch_exists(&checkout, &branch) {
        workspace = format!("{base_workspace}-{n}");
        branch = format!("{base_branch}-{n}");
        work_dir = home().join(format!("src/work-{workspace}")).join(&repo_name);
        session_label = format!("{prefix}{short_label} ({n})");
        n += 1;
    }

    // Theme: pick a palette color not already claimed by a tracked session,
    // then an emoji within it.
    let used = crate::sessions::used_colors();
    let pool: Vec<&str> = PALETTE.iter().copied().filter(|c| !used.contains(&c.to_string())).collect();
    let pool: &[&str] = if pool.is_empty() { PALETTE } else { &pool };
    let color = pool[hash_index(&workspace, 1, pool.len())];
    let emojis = palette_emojis(color);
    let emoji = emojis[hash_index(&workspace, 2, emojis.len())];
    let session_title = format!("{emoji} {session_label}");

    // Create the worktree from the repo's default branch.
    let work_parent = work_dir.parent().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    std::fs::create_dir_all(work_dir.parent().unwrap()).map_err(|e| e.to_string())?;
    git(&checkout, &["fetch", "origin", "--quiet"]).ok();
    git(&checkout, &["worktree", "add", &work_dir.to_string_lossy(), "-b", &branch, &format!("origin/{default_branch}")])?;
    git(&work_dir, &["branch", "--unset-upstream"]).ok();

    // Copy configured env files (relative to checkout) into the worktree.
    let mut warnings = Vec::new();
    for rel in &settings.env_files {
        let src = checkout.join(rel);
        if !src.is_file() {
            warnings.push(format!("env file not found, skipped: {rel}"));
            continue;
        }
        let dst = work_dir.join(rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        if let Err(e) = std::fs::copy(&src, &dst) {
            warnings.push(format!("could not copy env file {rel}: {e}"));
        }
    }

    // Assign the issue to the token's user and record the workspace in a
    // comment. Non-fatal: the worktree already exists, so failures only warn.
    match gh.authenticated_login().await {
        Ok(login) => {
            if let Err(e) = gh.add_assignees(&repo, issue_number, &[login]).await {
                warnings.push(format!("could not assign issue #{issue_number}: {e}"));
            }
            let body = format!(
                "🤖 Spawned a local workspace for this issue.\n\n\
                 - **GitHub Branch:** `{branch}`\n\
                 - **Local Directory:** `{}`\n\
                 - **Claude Session:** `{session_title}`\n",
                work_dir.display()
            );
            if let Err(e) = gh.create_comment(&repo, issue_number, &body).await {
                warnings.push(format!("could not comment on issue #{issue_number}: {e}"));
            }
        }
        Err(e) => warnings.push(format!("could not resolve token user for assignment: {e}")),
    }

    write_vscode_files(&work_dir, &work_parent, color, &session_title)?;

    // Record the session so mAIestro can track it (and so its color counts as
    // taken for the next spawn). Non-fatal: the worktree already exists.
    let session = crate::sessions::Session {
        id: workspace.clone(),
        repo: repo.clone(),
        issue_number,
        issue_url: issue_url.clone(),
        branch: branch.clone(),
        default_branch: default_branch.clone(),
        work_dir: work_dir.display().to_string(),
        checkout_dir: checkout.display().to_string(),
        session_title: session_title.clone(),
        color: color.to_string(),
        emoji: emoji.to_string(),
        hidden: None,
    };
    if let Err(e) = crate::sessions::save(&session) {
        warnings.push(format!("could not record session: {e}"));
    }

    open_vscode(&work_dir)?;

    Ok(SpawnResult {
        work_dir: work_dir.display().to_string(),
        branch,
        issue_url,
        reused: false,
        warnings,
    })
}

// ── Create-issue-and-spawn ──────────────────────────────────────────────────────

/// First existing `bin` found on $PATH.
fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(bin)).find(|p| p.is_file())
}

/// Resolve the `claude` binary: prefer $PATH, then common install locations
/// (the app's $PATH is minimal when launched at login, so fall back to disk).
fn claude_binary() -> PathBuf {
    which("claude")
        .or_else(|| {
            [
                home().join(".claude/local/claude"),
                PathBuf::from("/opt/homebrew/bin/claude"),
                PathBuf::from("/usr/local/bin/claude"),
            ]
            .into_iter()
            .find(|p| p.is_file())
        })
        .unwrap_or_else(|| PathBuf::from("claude"))
}

/// A short, trimmed preview of some output for diagnostic error messages.
fn snippet(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return "<empty>".into();
    }
    s.chars().take(240).collect()
}

/// Pull the inner `{title, body}` out of claude's reply text (which may wrap it
/// in code fences or prose) and extract a non-empty title plus body.
fn parse_issue_draft(text: &str) -> Result<(String, String), String> {
    let braces = text.find('{').zip(text.rfind('}')).filter(|(s, e)| e > s);
    let Some((start, end)) = braces else {
        // No JSON object: claude replied conversationally (e.g. asking the user
        // to clarify a vague idea). Surface that reply verbatim so the caller
        // can show it and let the user decide.
        return Err(text.trim().chars().take(400).collect());
    };
    let v: serde_json::Value = serde_json::from_str(&text[start..=end])
        .map_err(|e| format!("could not parse claude reply as JSON ({e}): {}", snippet(text)))?;
    let title = v["title"].as_str().unwrap_or("").trim().to_string();
    let body = v["body"].as_str().unwrap_or("").trim().to_string();
    if title.is_empty() {
        return Err("claude returned an empty title".into());
    }
    Ok((title, body))
}

/// Run Claude (haiku) headlessly with `prompt`, in `dir` for repo context, and
/// return its reply text. Uses `--output-format json` so we parse a stable
/// envelope rather than guessing at raw text, and surfaces stdout/stderr in
/// errors when something goes wrong.
///
/// `--tools ""` disables ALL tools: these calls only need to generate text, so
/// the model must not be able to read arbitrary files, run Bash, edit, or fetch
/// URLs — even though it runs in the real checkout/worktree with the user's
/// ambient permissions. That contains prompt injection from the input text (or
/// from repo files like CLAUDE.md, which is still loaded as context) to, at
/// worst, a bad title/body the user reviews — not code execution or exfiltration.
async fn claude_text(dir: &Path, prompt: &str, what: &str) -> Result<String, String> {
    let run = tokio::process::Command::new(claude_binary())
        .current_dir(dir)
        .args(["-p", prompt, "--model", "haiku", "--output-format", "json", "--tools", ""])
        .output();
    let output = tokio::time::timeout(std::time::Duration::from_secs(90), run)
        .await
        .map_err(|_| format!("claude timed out while {what}"))?
        .map_err(|e| format!("could not run claude (is it installed and on PATH?): {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(format!("claude exited with an error: {}", snippet(&stderr)));
    }

    // `--output-format json` wraps the reply in a result envelope.
    let envelope: serde_json::Value = serde_json::from_str(stdout.trim()).map_err(|e| {
        format!("could not parse claude output ({e}); stdout: {}; stderr: {}", snippet(&stdout), snippet(&stderr))
    })?;
    if envelope["is_error"].as_bool().unwrap_or(false) {
        return Err(format!("claude reported an error: {}", snippet(envelope["result"].as_str().unwrap_or(""))));
    }
    Ok(envelope["result"].as_str().unwrap_or("").to_string())
}

/// Ask Claude (haiku), running in the repo checkout for context, to turn the
/// user's free-text idea into an issue title + markdown body.
async fn draft_issue(checkout: &Path, idea: &str) -> Result<(String, String), String> {
    let prompt = format!(
        "Based on this idea for a change to this codebase, draft a GitHub issue. \
         Reply with ONLY a JSON object of the form {{\"title\": string, \"body\": string}}. \
         The title is a concise summary (max ~70 characters). The body is clear markdown \
         describing the work. Idea: {idea}"
    );
    let reply = claude_text(checkout, &prompt, "drafting the issue").await?;
    parse_issue_draft(&reply)
}

/// A drafted issue ready to create, or a signal that Claude couldn't produce a
/// clear draft and the user must confirm creating from raw text.
enum DraftStep {
    Ready {
        title: String,
        body: String,
        /// Non-fatal note to surface alongside the created issue.
        warning: Option<String>,
    },
    NeedsConfirmation { message: String },
}

/// Resolve the repo's identity + checkout, then turn the idea into an issue
/// draft — via Claude, or (when `use_raw_fallback`) straight from the raw text.
/// Returns the authenticated GitHub client alongside the draft so callers can
/// create the issue. Shared by `create_issue` and `create_issue_and_spawn`.
async fn resolve_draft(
    repo: &str,
    idea: &str,
    use_raw_fallback: bool,
) -> Result<(GitHub, DraftStep), String> {
    let idea = idea.trim();
    if idea.is_empty() {
        return Err("Describe what you want to work on first.".into());
    }

    let settings = crate::repo_settings::repo_settings_get(repo.to_string());
    let identity_id = settings
        .identity_id
        .clone()
        .ok_or_else(|| "No identity assigned to this repo. Set one in Settings → Repo.".to_string())?;
    let checkout = expand_tilde(settings.checkout_dir.as_deref().unwrap_or_default());
    if !checkout.join(".git").exists() {
        return Err(format!("checkout dir is not a git repo: {}", checkout.display()));
    }
    let gh = GitHub::for_identity(&identity_id)?;

    let step = if use_raw_fallback {
        DraftStep::Ready {
            title: trim_to_word(idea, 70),
            body: idea.to_string(),
            warning: Some("created from your text without an AI draft".to_string()),
        }
    } else {
        match draft_issue(&checkout, idea).await {
            Ok((title, body)) => DraftStep::Ready { title, body, warning: None },
            // Couldn't draft: let the user confirm before creating anything.
            Err(message) => DraftStep::NeedsConfirmation { message },
        }
    };
    Ok((gh, step))
}

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
    repo: String,
    idea: String,
    use_raw_fallback: bool,
) -> Result<CreateIssueOutcome, String> {
    let (gh, step) = resolve_draft(&repo, &idea, use_raw_fallback).await?;
    let (title, body, warning) = match step {
        DraftStep::Ready { title, body, warning } => (title, body, warning),
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

/// Draft an issue from the user's idea (via Claude), open it on GitHub, then
/// spawn a workspace for the freshly created issue. See `create_issue` for the
/// drafting / needs-confirmation semantics.
#[tauri::command]
pub async fn create_issue_and_spawn(
    repo: String,
    idea: String,
    use_raw_fallback: bool,
    force_new: bool,
) -> Result<CreateAndSpawnOutcome, String> {
    let (gh, step) = resolve_draft(&repo, &idea, use_raw_fallback).await?;
    let (title, body, draft_warning) = match step {
        DraftStep::Ready { title, body, warning } => (title, body, warning),
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

// ── Teardown ────────────────────────────────────────────────────────────────────

/// Close the VS Code window(s) for this worktree by pressing each matching
/// window's native close button via the accessibility API. We deliberately use
/// System Events here (the same path `focus_editor_window` uses) rather than
/// direct Apple events to "Visual Studio Code": Electron's scripting suite is
/// unreliable, and the direct-events path also needs a *separate* Automation
/// grant that we'd never prompted for — so the close was failing silently.
/// No-op if VS Code isn't running.
async fn close_editor_window(marker: &str) {
    let safe = marker.replace('"', "");
    let script = format!(
        r#"tell application "System Events"
  if not (exists process "Code") then return
  tell process "Code"
    repeat with w in windows
      if name of w contains "{safe}" then
        try
          perform action "AXPress" of (first button of w whose subrole is "AXCloseButton")
        end try
      end if
    end repeat
  end tell
end tell"#
    );
    let _ = tokio::process::Command::new("osascript").arg("-e").arg(&script).output().await;
}

/// What we could learn about a worktree's VS Code window. The `Denied` case is
/// critical: when mAIestro lacks Accessibility permission, osascript errors and
/// we genuinely cannot see the window — which must NOT be mistaken for "closed",
/// or teardown would delete the folder out from under a live VS Code and crash it.
enum WinProbe {
    /// A window whose title contains the marker is open.
    Open,
    /// VS Code isn't running, or no window matches the marker.
    Absent,
    /// Couldn't determine — almost always a missing Accessibility grant.
    Denied,
}

/// Probe for an open VS Code window whose title contains `marker`, via the
/// accessibility API (System Events).
async fn probe_editor_window(marker: &str) -> WinProbe {
    let safe = marker.replace('"', "");
    let script = format!(
        r#"tell application "System Events"
  if not (exists process "Code") then return "absent"
  tell process "Code"
    repeat with w in windows
      if name of w contains "{safe}" then return "open"
    end repeat
  end tell
end tell
return "absent""#
    );
    match tokio::process::Command::new("osascript").arg("-e").arg(&script).output().await {
        Ok(out) if out.status.success() => {
            match String::from_utf8_lossy(&out.stdout).trim() {
                "open" => WinProbe::Open,
                _ => WinProbe::Absent,
            }
        }
        // Non-zero exit (e.g. "-25211 not allowed assistive access") or spawn failure.
        _ => WinProbe::Denied,
    }
}

/// Permission-free safety net: is any process's working directory inside this
/// worktree? Our spawned `claude` runs in VS Code's integrated terminal with its
/// cwd in the worktree, so this catches the common "still open" case without
/// needing Accessibility. Uses `lsof -d cwd` (process CWDs only) to avoid the
/// slow tree walk that `lsof +D` would do over a full checkout.
fn worktree_in_use(work_dir: &Path) -> bool {
    let dir = work_dir.to_string_lossy();
    match Command::new("lsof").args(["-d", "cwd", "-Fn"]).output() {
        Ok(out) => String::from_utf8_lossy(&out.stdout)
            .lines()
            .any(|l| l.strip_prefix('n').is_some_and(|p| p.starts_with(&*dir))),
        Err(_) => false,
    }
}

/// Open System Settings → Privacy & Security → Accessibility so the user can
/// grant mAIestro the permission teardown needs to close VS Code windows.
fn open_accessibility_settings() {
    let _ = Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn();
}

#[derive(serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TeardownOutcome {
    Done,
    /// Checks found unresolved work; `warnings` describes it so the UI can ask
    /// the user to confirm before destroying the worktree.
    NeedsConfirmation { warnings: Vec<String> },
}

/// Tear down a spawned session's worktree. Inspects the branch first
/// (uncommitted changes, PR state, unmerged commits); confirmation is required
/// in every case except when the PR is merged and nothing new remains. On
/// teardown the VS Code window is closed *first* (open windows have caused
/// removal failures), then the worktree, local branch, directory, and session
/// record are removed.
#[tauri::command]
pub async fn teardown(session_id: String, confirmed: bool) -> Result<TeardownOutcome, String> {
    let session = crate::sessions::get(&session_id)
        .ok_or_else(|| format!("session not found: {session_id}"))?;
    let work_dir = PathBuf::from(&session.work_dir);
    let checkout = expand_tilde(&session.checkout_dir);
    let branch = session.branch.clone();
    let base = session.default_branch.clone();

    // ── Checks ──────────────────────────────────────────────────────────────
    let mut warnings = Vec::new();

    let dirty = git(&work_dir, &["status", "--porcelain"])
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    if dirty {
        warnings.push("Worktree has uncommitted changes".to_string());
    }

    // PR state via the REST API (best-effort: needs an identity + token).
    let mut pr_merged = false;
    let settings = crate::repo_settings::repo_settings_get(session.repo.clone());
    if let Some(identity_id) = settings.identity_id {
        if let Ok(gh) = GitHub::for_identity(&identity_id) {
            if let Ok(prs) = gh.pulls_for_branch(&session.repo, &branch).await {
                pr_merged = prs.iter().any(|p| p["merged_at"].is_string());
                if let Some(open) = prs.iter().find(|p| p["state"].as_str() == Some("open")) {
                    warnings.push(format!("PR #{} is still open", open["number"].as_u64().unwrap_or(0)));
                }
            }
        }
    }

    // Commits on the branch not yet on the base, when no merged PR accounts for them.
    let ahead = git(&work_dir, &["rev-list", "--count", &format!("origin/{base}..{branch}")])
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
    if let Some(marker) = window_marker(&work_dir) {
        close_editor_window(&marker).await;
        let mut waited = 0u64;
        loop {
            match probe_editor_window(&marker).await {
                // Window confirmed gone — safe to delete.
                WinProbe::Absent => break,
                // No Accessibility grant: we can neither close nor see the window.
                // Fall back to the permission-free check — if nothing is using the
                // worktree, proceed; otherwise stop and guide the user.
                WinProbe::Denied => {
                    if worktree_in_use(&work_dir) {
                        open_accessibility_settings();
                        return Err(
                            "mAIestro needs Accessibility permission to close the VS Code window \
                             before removing this worktree (without it, VS Code crashes). I opened \
                             System Settings → Privacy & Security → Accessibility — enable mAIestro \
                             there and try again, or just close the VS Code window yourself first."
                                .to_string(),
                        );
                    }
                    break;
                }
                // Window still open: the close is in flight (or we lack permission
                // to close but the user may close it). Wait a bit, then give up.
                WinProbe::Open => {
                    if waited >= 4000 {
                        return Err(
                            "VS Code still has this worktree open — close its window, then try Clean Up again."
                                .to_string(),
                        );
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    waited += 300;
                }
            }
        }
    }

    // 2. Remove the worktree (force: the user confirmed discarding any changes).
    git(&checkout, &["worktree", "remove", "--force", &work_dir.to_string_lossy()])?;

    // 3. Delete the local branch (-D: spawn unset the upstream and -d checks the
    //    wrong base, so it would refuse even for merged branches).
    if local_branch_exists(&checkout, &branch) {
        let _ = git(&checkout, &["branch", "-D", &branch]);
    }

    // 4. Remove the leftover work-* parent dir, guarding the path shape.
    if let Some(parent) = work_dir.parent() {
        let src = home().join("src");
        let under_src = parent.parent() == Some(src.as_path());
        let is_work = parent.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("work-"));
        if under_src && is_work && parent.exists() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    // 5. Drop the session record.
    let _ = crate::sessions::delete(&session_id);

    Ok(TeardownOutcome::Done)
}

// ── Session PR link ───────────────────────────────────────────────────────────

/// A pull request associated with a session's branch, surfaced to the UI as a
/// clickable link in the session pill.
#[derive(serde::Serialize)]
pub struct PrLink {
    pub number: u64,
    pub html_url: String,
    pub title: String,
    /// One of "draft", "open", "merged", "closed".
    pub state: String,
}

/// Collapse GitHub's `state` / `draft` / `merged_at` fields into a single label.
fn pr_state(pr: &serde_json::Value) -> String {
    if pr["merged_at"].is_string() {
        "merged".to_string()
    } else if pr["state"].as_str() == Some("open") {
        if pr["draft"].as_bool().unwrap_or(false) {
            "draft".to_string()
        } else {
            "open".to_string()
        }
    } else {
        "closed".to_string()
    }
}

/// The pull request to show for a session's branch, if any. Prefers the most
/// recently created open PR; otherwise the most recently created PR of any
/// state, so the link still resolves after the PR is merged or closed.
///
/// Returns `Ok(None)` — not an error — when the session is gone, the repo has
/// no identity configured, or the branch has no PRs. The UI treats all of these
/// the same: it simply renders no PR button. Only an actual API failure (e.g.
/// auth/network) surfaces as `Err`, which the frontend also degrades silently.
#[tauri::command]
pub async fn session_pr(session_id: String) -> Result<Option<PrLink>, String> {
    let Some(session) = crate::sessions::get(&session_id) else {
        return Ok(None);
    };
    let settings = crate::repo_settings::repo_settings_get(session.repo.clone());
    let Some(identity_id) = settings.identity_id else {
        return Ok(None);
    };
    let gh = GitHub::for_identity(&identity_id)?;
    let prs = gh.pulls_for_branch(&session.repo, &session.branch).await?;

    // `created_at` is ISO-8601, so lexicographic order is chronological.
    let created_at = |p: &&serde_json::Value| p["created_at"].as_str().unwrap_or("").to_string();
    let best = prs
        .iter()
        .filter(|p| p["state"].as_str() == Some("open"))
        .max_by_key(created_at)
        .or_else(|| prs.iter().max_by_key(created_at));

    Ok(best.map(|pr| PrLink {
        number: pr["number"].as_u64().unwrap_or(0),
        html_url: pr["html_url"].as_str().unwrap_or("").to_string(),
        title: pr["title"].as_str().unwrap_or("").to_string(),
        state: pr_state(pr),
    }))
}

// ── Create PR ───────────────────────────────────────────────────────────────────

fn pr_link_from(pr: &serde_json::Value) -> PrLink {
    PrLink {
        number: pr["number"].as_u64().unwrap_or(0),
        html_url: pr["html_url"].as_str().unwrap_or("").to_string(),
        title: pr["title"].as_str().unwrap_or("").to_string(),
        state: pr_state(pr),
    }
}

/// Build a context blob describing the branch's changes for the PR drafter: the
/// commit log plus the diff against the base, capped so a huge diff falls back
/// to a file-level `--stat` rather than blowing past the prompt budget.
fn change_summary(work_dir: &Path, base: &str) -> String {
    let range = format!("origin/{base}..HEAD");
    let log = git(work_dir, &["log", "--oneline", &range]).unwrap_or_default();
    let diff = git(work_dir, &["diff", &format!("origin/{base}...HEAD")]).unwrap_or_default();
    const MAX_DIFF: usize = 12_000;
    let diff_section = if diff.chars().count() > MAX_DIFF {
        let stat = git(work_dir, &["diff", "--stat", &format!("origin/{base}...HEAD")]).unwrap_or_default();
        format!("Diff too large to include in full; file-level summary:\n{stat}")
    } else {
        diff
    };
    format!("Commits:\n{log}\n\nDiff:\n{diff_section}")
}

/// Create a draft pull request for a session's branch, with a Claude-drafted
/// title and description. Pushes the branch to origin first (mAIestro's own
/// local-git op, like `git worktree add` — the launched session's own pushes are
/// separate), reuses an already-open PR instead of duplicating, and links the PR
/// to the originating issue with `Closes #N`.
#[tauri::command]
pub async fn session_create_pr(session_id: String) -> Result<PrLink, String> {
    let session = crate::sessions::get(&session_id)
        .ok_or_else(|| format!("session not found: {session_id}"))?;
    let work_dir = PathBuf::from(&session.work_dir);
    let base = session.default_branch.clone();
    let branch = session.branch.clone();

    // Guard: uncommitted changes wouldn't make it into the PR (it's built from the
    // pushed branch), so block and ask the user to commit them first.
    let dirty = git(&work_dir, &["status", "--porcelain"])
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    if dirty {
        return Err("This worktree has uncommitted changes. Commit them first, then create the PR.".to_string());
    }

    let settings = crate::repo_settings::repo_settings_get(session.repo.clone());
    let identity_id = settings
        .identity_id
        .ok_or_else(|| "No identity assigned to this repo. Set one in Settings → Repo.".to_string())?;
    let gh = GitHub::for_identity(&identity_id)?;

    // Refresh the base ref so the ahead-count and diff compare against current origin.
    git(&work_dir, &["fetch", "origin", &base, "--quiet"]).ok();

    // Guard: nothing to open a PR for.
    let ahead = git(&work_dir, &["rev-list", "--count", &format!("origin/{base}..HEAD")])
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);
    if ahead == 0 {
        return Err(format!("No commits on this branch ahead of {base} to open a PR for."));
    }

    // Reuse an existing open/draft PR instead of creating a duplicate.
    if let Ok(prs) = gh.pulls_for_branch(&session.repo, &branch).await {
        if let Some(open) = prs.iter().find(|p| p["state"].as_str() == Some("open")) {
            return Ok(pr_link_from(open));
        }
    }

    // Push the branch so GitHub can see the head ref. -u sets upstream for the
    // user's later pushes from the session.
    git(&work_dir, &["push", "-u", "origin", &branch])
        .map_err(|e| format!("could not push branch {branch}: {e}"))?;

    // Seed the draft with the issue title/body for context (best-effort).
    let issue = gh.issue(&session.repo, session.issue_number).await.unwrap_or_default();
    let issue_title = issue["title"].as_str().unwrap_or("").to_string();
    let issue_body = issue["body"].as_str().unwrap_or("");

    let summary = change_summary(&work_dir, &base);
    let prompt = format!(
        "Draft a GitHub pull request description for the changes below. Reply with ONLY a \
         JSON object of the form {{\"title\": string, \"body\": string}}. The title is a \
         concise summary of the overall change (max ~70 characters). The body is clear \
         markdown explaining what changed and why; do not include a heading that repeats the \
         title. This PR resolves issue #{number} (\"{issue_title}\"). \
         \n\nOriginating issue body:\n{issue_body}\n\nChanges:\n{summary}",
        number = session.issue_number,
    );

    // Draft via Claude; fall back to the issue title + change summary if it fails
    // so the action still produces a usable PR.
    let (title, body) = match claude_text(&work_dir, &prompt, "drafting the PR").await {
        Ok(reply) => parse_issue_draft(&reply).unwrap_or_else(|_| {
            (fallback_title(&issue_title, &branch), summary.clone())
        }),
        Err(_) => (fallback_title(&issue_title, &branch), summary.clone()),
    };

    let body = format!("{body}\n\nCloses #{}", session.issue_number);

    let pr = gh
        .create_pull(&session.repo, &title, &branch, &base, &body, true)
        .await?;
    Ok(pr_link_from(&pr))
}

/// Title to use when the AI draft is unavailable: the tracked issue title, or
/// the branch name as a last resort.
fn fallback_title(issue_title: &str, branch: &str) -> String {
    if issue_title.trim().is_empty() {
        branch.to_string()
    } else {
        issue_title.to_string()
    }
}
