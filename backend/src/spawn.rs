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

fn write_vscode_files(work_dir: &Path, work_parent: &str, color: &str) -> Result<(), String> {
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
    // remotely; mAIestro still only launches it, it does not host it.
    let tasks = serde_json::json!({
        "version": "2.0.0",
        "tasks": [{
            "label": "Start Claude",
            "type": "shell",
            "command": "claude --remote-control",
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

fn open_vscode(dir: &Path) -> Result<(), String> {
    // Launch Services hand-off, per the "all launches use open -a" decision.
    Command::new("open")
        .args(["-a", "Visual Studio Code"])
        .arg(dir)
        .spawn()
        .map_err(|e| format!("failed to open VS Code: {e}"))?;
    Ok(())
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

    write_vscode_files(&work_dir, &work_parent, color)?;

    // Record the session so mAIestro can track it (and so its color counts as
    // taken for the next spawn). Non-fatal: the worktree already exists.
    let session = crate::sessions::Session {
        id: workspace.clone(),
        repo: repo.clone(),
        issue_number,
        issue_url: issue_url.clone(),
        branch: branch.clone(),
        work_dir: work_dir.display().to_string(),
        checkout_dir: checkout.display().to_string(),
        session_title: session_title.clone(),
        color: color.to_string(),
        emoji: emoji.to_string(),
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
