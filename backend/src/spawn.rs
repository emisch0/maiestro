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

/// Choose a palette color not already claimed by a tracked session (falling back
/// to the full palette when all are taken), then an emoji within it. Seeded by
/// `seed` so the same workspace name themes consistently.
fn pick_theme(seed: &str) -> (&'static str, &'static str) {
    let used = crate::sessions::used_colors();
    let pool: Vec<&'static str> = PALETTE.iter().copied().filter(|c| !used.contains(&c.to_string())).collect();
    let pool: Vec<&'static str> = if pool.is_empty() { PALETTE.to_vec() } else { pool };
    let color = pool[hash_index(seed, 1, pool.len())];
    let emojis = palette_emojis(color);
    let emoji = emojis[hash_index(seed, 2, emojis.len())];
    (color, emoji)
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

/// A reasonable default short label from a (possibly long) issue title when no
/// AI-generated or user-edited label is available: trimmed to a word boundary
/// and stripped of trailing punctuation. Never empty.
fn default_short_title(title: &str) -> String {
    let s = trim_to_word(title, 50);
    let s = s.trim_end_matches(|c: char| !c.is_alphanumeric()).trim();
    if s.is_empty() { "work".to_string() } else { s.to_string() }
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

// ── Claude Code hooks (live session status) ───────────────────────────────────

/// Write Claude Code hooks into the worktree's `.claude/settings.local.json`
/// (the personal, gitignored layer that merges with the user's settings and
/// applies to both terminal and VS Code integrated-terminal sessions). Each hook
/// invokes *this* binary as `maiestro hook <state> --workspace <ws-id>`, which
/// writes a status record the backend watches. See `status.rs`.
///
/// Merges into any existing file rather than overwriting, so user/repo settings
/// and unrelated hooks survive.
fn write_claude_hooks(work_dir: &Path, ws_id: &str) -> Result<(), String> {
    let bin = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;

    let dir = work_dir.join(".claude");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("settings.local.json");

    // Start from any existing settings, then merge ours in (replacing any prior
    // entries of ours so a changed binary path heals rather than duplicating).
    let root: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .filter(|v: &serde_json::Value| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    let root = merge_hooks(root, &bin, ws_id);

    std::fs::write(&path, serde_json::to_string_pretty(&root).unwrap() + "\n")
        .map_err(|e| e.to_string())?;

    exclude_generated_files(work_dir);
    Ok(())
}

/// Build mAIestro's status-hook entries (event name → hook group) for a worktree,
/// using `bin` as the helper binary path. Shared by spawn (which writes them) and
/// startup reconcile (which rewrites them at the current binary). The commands run
/// through a shell, so the binary path (may contain spaces, e.g. inside
/// "/Applications/.../mAIestro.app") and the ws id are single-quoted.
fn maiestro_hook_groups(bin: &Path, ws_id: &str) -> Vec<(&'static str, serde_json::Value)> {
    let bin_q = shell_quote(&bin.to_string_lossy());
    let ws_q = shell_quote(ws_id);
    let cmd = |state: &str| format!("{bin_q} hook {state} --workspace {ws_q}");

    // Bare hook group (events that take no matcher).
    let group = |state: &str| {
        serde_json::json!({
            "hooks": [{ "type": "command", "command": cmd(state) }]
        })
    };
    // PreToolUse takes a matcher group; "" matches all tools.
    let matcher_group = |state: &str| {
        serde_json::json!({
            "matcher": "",
            "hooks": [{ "type": "command", "command": cmd(state) }]
        })
    };

    vec![
        ("SessionStart", group("running")),
        // Its own `prompt` verb (not `busy`) so a fresh turn clears a stale
        // failed-tool error while still reading as working.
        ("UserPromptSubmit", group("prompt")),
        ("PreToolUse", matcher_group("busy")),
        // PostToolUse is the event that fires *after* an approved permission
        // prompt's tool completes — the only signal that Claude has resumed
        // working. Without it, a session sticks on `needs_you` (from the prompt's
        // Notification) all the way through the rest of the turn, even while
        // Claude is actively thinking. PreToolUse alone can't cover this: it
        // fires *before* the prompt, not after approval. The distinct `tool_ok`
        // verb (vs PreToolUse's `busy`) also marks a tool *succeeding*, which
        // clears a pending transient `last_error` (issue #48); it still reads as
        // `busy`.
        ("PostToolUse", matcher_group("tool_ok")),
        // A failed tool call: captures the error into `last_error` (kept until
        // dismissed) and logs it. State stays `busy` — Claude works on past it.
        ("PostToolUseFailure", matcher_group("tool_failed")),
        ("Notification", group("notification")),
        ("Stop", group("idle")),
        ("SessionEnd", group("ended")),
    ]
}

/// True when `command` is one of mAIestro's status hooks for `ws_id` — matched by
/// the trailing `--workspace '<ws-id>'` we always emit, so unrelated hooks (and
/// other workspaces' hooks) in the same file are left untouched.
fn is_maiestro_hook(command: &str, ws_id: &str) -> bool {
    command.contains(" hook ") && command.contains(&format!("--workspace {}", shell_quote(ws_id)))
}

/// True when a hook *group* contains a command that's one of ours for `ws_id`.
fn group_is_ours(group: &serde_json::Value, ws_id: &str) -> bool {
    group["hooks"]
        .as_array()
        .is_some_and(|hooks| hooks.iter().any(|h| h["command"].as_str().is_some_and(|c| is_maiestro_hook(c, ws_id))))
}

/// True when a parsed settings root already carries any of our hooks for `ws_id`.
fn has_maiestro_hooks(root: &serde_json::Value, ws_id: &str) -> bool {
    root["hooks"].as_object().is_some_and(|events| {
        events
            .values()
            .any(|arr| arr.as_array().is_some_and(|gs| gs.iter().any(|g| group_is_ours(g, ws_id))))
    })
}

/// Merge mAIestro's hooks into a parsed settings `root`: for each event, drop any
/// existing entries that are ours (stale paths from a prior spawner), then append
/// a fresh group built from `bin`. Every other hook and setting is preserved.
fn merge_hooks(mut root: serde_json::Value, bin: &Path, ws_id: &str) -> serde_json::Value {
    if !root.is_object() {
        root = serde_json::json!({});
    }
    let mut hooks = serde_json::Value::Object(root["hooks"].as_object().cloned().unwrap_or_default());
    for (event, group) in maiestro_hook_groups(bin, ws_id) {
        let mut arr = hooks[event].as_array().cloned().unwrap_or_default();
        arr.retain(|g| !group_is_ours(g, ws_id));
        arr.push(group);
        hooks[event] = serde_json::Value::Array(arr);
    }
    root["hooks"] = hooks;
    root
}

/// Rewrite a tracked session's status hooks to point at the *currently running*
/// binary, healing a stale `current_exe()` path baked in by a spawner that has
/// since been torn down or rebuilt (issue #35). Best-effort and quiet:
///
/// - No-op if the worktree or its `.claude/settings.local.json` is gone.
/// - No-op if the file carries none of our hooks (we never inject into a worktree
///   that didn't already have them).
/// - Writes only when the resulting JSON actually changed, so it doesn't churn
///   the file on every launch.
///
/// Returns true when it rewrote the file.
fn reconcile_session_hooks(work_dir: &Path, ws_id: &str) -> bool {
    let path = work_dir.join(".claude").join("settings.local.json");
    let Ok(existing) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(root) = serde_json::from_str::<serde_json::Value>(&existing) else {
        return false;
    };
    if !root.is_object() || !has_maiestro_hooks(&root, ws_id) {
        return false;
    }
    let Ok(bin) = std::env::current_exe() else {
        return false;
    };
    let updated = serde_json::to_string_pretty(&merge_hooks(root, &bin, ws_id)).unwrap() + "\n";
    if updated == existing {
        return false;
    }
    std::fs::write(&path, updated).is_ok()
}

/// At startup, heal stale hook binary paths across every tracked session (see
/// `reconcile_session_hooks`). Logs how many sessions were rewritten.
pub fn reconcile_all_session_hooks() {
    let fixed = crate::sessions::load_all()
        .into_iter()
        .filter(|s| reconcile_session_hooks(Path::new(&s.work_dir), &s.id))
        .count();
    if fixed > 0 {
        tracing::info!(sessions = fixed, "reconciled stale status-hook paths");
    }
}

/// Append mAIestro's generated files to the worktree's shared git exclude file so
/// they don't show up as untracked changes (which would trip teardown's
/// `git status --porcelain` dirty check before Claude has run / in repos that
/// don't already ignore them). Idempotent and best-effort.
fn exclude_generated_files(work_dir: &Path) {
    // Worktrees share the main repo's exclude via the common git dir; resolve it
    // rather than assuming `<work_dir>/.git` is a directory (in a worktree it's a
    // file pointing elsewhere).
    let Ok(common) = git(work_dir, &["rev-parse", "--git-common-dir"]) else {
        return;
    };
    let common = expand_tilde(&common);
    let common = if common.is_absolute() { common } else { work_dir.join(common) };
    let exclude = common.join("info").join("exclude");

    let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
    let mut to_add: Vec<&str> = Vec::new();
    for pat in [".claude/settings.local.json", ".vscode/"] {
        if !existing.lines().any(|l| l.trim() == pat) {
            to_add.push(pat);
        }
    }
    if to_add.is_empty() {
        return;
    }
    if let Some(parent) = exclude.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut body = existing;
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str("# Added by mAIestro\n");
    for pat in to_add {
        body.push_str(pat);
        body.push('\n');
    }
    let _ = std::fs::write(&exclude, body);
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
    crate::log_invoke!("open_in_editor", work_dir = %work_dir);
    let path = PathBuf::from(&work_dir);
    if let Some(marker) = window_marker(&path) {
        if focus_editor_window(&marker).await {
            return Ok(());
        }
    }
    open_vscode(&path)
}

/// Open a tracked repo's main checkout directory in VS Code. Unlike
/// `open_in_editor` this is a pure launch — no worktree, no session, no status —
/// reusing the same `open_vscode` path logic as spawned worktrees.
#[tauri::command]
pub async fn open_repo_in_editor(repo: String) -> Result<(), String> {
    crate::log_invoke!("open_repo_in_editor", repo = %repo);
    let settings = crate::repo_settings::repo_settings_get(repo)?;
    let checkout = expand_tilde(settings.checkout_dir.as_deref().unwrap_or_default());
    if !checkout.join(".git").exists() {
        return Err(format!("checkout dir is not a git repo: {}", checkout.display()));
    }
    open_vscode(&checkout)
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

/// Core worktree + session creation, shared by every spawn path. Resolves the
/// repo's settings/identity/checkout itself; the caller supplies the issue facts
/// and the (reviewed) label/theming. The slug is `<n>-<slug(short_label)>`.
#[tracing::instrument(skip_all, fields(session = tracing::field::Empty))]
async fn do_spawn(d: SpawnDecision<'_>) -> Result<SpawnResult, String> {
    let SpawnDecision { repo, issue_number, issue_url, default_branch, short_label, color, emoji, force_new } = d;

    let settings = crate::repo_settings::repo_settings_get(repo.to_string())?;
    let identity_id = settings
        .identity_id
        .clone()
        .ok_or_else(|| "No identity assigned to this repo. Set one in Settings → Repo.".to_string())?;
    let checkout = expand_tilde(settings.checkout_dir.as_deref().unwrap_or_default());
    if !checkout.join(".git").exists() {
        return Err(format!("checkout dir is not a git repo: {}", checkout.display()));
    }
    let repo_name = repo.split('/').next_back().unwrap_or(repo).to_string();
    let gh = GitHub::for_identity(&identity_id)?;

    let short_label = {
        let t = short_label.trim();
        if t.is_empty() { default_short_title("") } else { t.to_string() }
    };

    // Worktree location prefix: the full path is `<prefix><workspace>/<repo>`
    // (string concat — the trailing `work-` is part of the dir name). Unset
    // falls back to the original `~/src/work-` behavior.
    let worktree_prefix = settings.worktree_prefix.as_deref().unwrap_or("~/src/work-").to_string();
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
    while work_dir.is_dir() || local_branch_exists(&checkout, &branch) {
        workspace = format!("{base_workspace}-{n}");
        branch = format!("{base_branch}-{n}");
        work_dir = worktree_dir(&workspace);
        session_label = format!("{prefix}{short_label} ({n})");
        n += 1;
    }
    // Re-record once the final (possibly suffixed) workspace id is resolved.
    tracing::Span::current().record("session", workspace.as_str());

    let session_title = format!("{emoji} {session_label}");

    // Create the worktree from the repo's default branch.
    let work_parent = work_dir.parent().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    std::fs::create_dir_all(work_dir.parent().unwrap()).map_err(|e| e.to_string())?;
    if let Err(e) = git(&checkout, &["fetch", "origin", "--quiet"]) {
        tracing::warn!(error = %e, "git fetch before spawn failed (continuing)");
    }
    git(&checkout, &["worktree", "add", &work_dir.to_string_lossy(), "-b", &branch, &format!("origin/{default_branch}")])?;
    if let Err(e) = git(&work_dir, &["branch", "--unset-upstream"]) {
        tracing::warn!(error = %e, "git branch --unset-upstream failed (continuing)");
    }

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
            if let Err(e) = gh.add_assignees(repo, issue_number, &[login]).await {
                warnings.push(format!("could not assign issue #{issue_number}: {e}"));
            }
            let body = format!(
                "🤖 Spawned a local workspace for this issue.\n\n\
                 - **GitHub Branch:** `{branch}`\n\
                 - **Local Directory:** `{}`\n\
                 - **Claude Session:** `{session_title}`\n",
                work_dir.display()
            );
            if let Err(e) = gh.create_comment(repo, issue_number, &body).await {
                warnings.push(format!("could not comment on issue #{issue_number}: {e}"));
            }
        }
        Err(e) => warnings.push(format!("could not resolve token user for assignment: {e}")),
    }

    write_vscode_files(&work_dir, &work_parent, color, &session_title)?;
    write_claude_hooks(&work_dir, &workspace)?;

    // Record the session so mAIestro can track it (and so its color counts as
    // taken for the next spawn). Non-fatal: the worktree already exists.
    let session = crate::sessions::Session {
        id: workspace.clone(),
        repo: repo.to_string(),
        issue_number,
        issue_url: issue_url.to_string(),
        branch: branch.clone(),
        default_branch: default_branch.to_string(),
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

    tracing::info!(repo = %repo, issue = issue_number, branch = %branch, reused = false, "spawned workspace");
    Ok(SpawnResult {
        work_dir: work_dir.display().to_string(),
        branch,
        issue_url: issue_url.to_string(),
        reused: false,
        warnings,
    })
}

/// Fetch an issue's facts (title, url) and the repo's default branch — the
/// shared first step of preparing or running a spawn for an existing issue.
async fn issue_facts(gh: &GitHub, repo: &str, issue_number: u64) -> Result<(String, String, String), String> {
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
    let settings = crate::repo_settings::repo_settings_get(repo.clone())?;
    let identity_id = settings
        .identity_id
        .ok_or_else(|| "No identity assigned to this repo. Set one in Settings → Repo.".to_string())?;
    let gh = GitHub::for_identity(&identity_id)?;
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

/// Pull the inner `{title, body, short_title}` out of claude's reply text (which
/// may wrap it in code fences or prose) and extract a non-empty title, body, and
/// a short branch-friendly label. Falls back to the title for `short_title` when
/// the model omits it.
fn parse_issue_draft(text: &str) -> Result<(String, String, String), String> {
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
    let short_title = v["short_title"].as_str().unwrap_or("").trim().to_string();
    let short_title = if short_title.is_empty() { default_short_title(&title) } else { short_title };
    Ok((title, body, short_title))
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
/// user's free-text idea into an issue title + markdown body + a short label.
/// The `short_title` is produced in the *same* call (no extra Claude run): it's
/// a punchy branch/session label, distinct from the full issue title.
async fn draft_issue(checkout: &Path, idea: &str) -> Result<(String, String, String), String> {
    let prompt = format!(
        "Based on this idea for a change to this codebase, draft a GitHub issue. \
         Reply with ONLY a JSON object of the form \
         {{\"title\": string, \"body\": string, \"short_title\": string}}. \
         The title is a concise summary (max ~70 characters). The body is clear markdown \
         describing the work. The short_title is a short, human-readable session label — \
         plain words with normal spaces and capitalization (NOT a slug or branch name, so \
         no dashes/underscores), at most ~5 words / 40 characters, no issue number, no \
         trailing punctuation; it should read well, not just be the title cut off. Idea: {idea}"
    );
    let reply = claude_text(checkout, &prompt, "drafting the issue").await?;
    parse_issue_draft(&reply)
}

/// Pull a usable short label out of Claude's reply to `suggest_short_label`.
/// The prompt asks for the bare label, but the model may still wrap it in
/// quotes, backticks, or a code fence — strip those. A multi-line or over-long
/// reply means it rambled instead of labeling: error, the caller keeps the
/// heuristic label.
fn parse_short_label(text: &str) -> Result<String, String> {
    let mut lines = text.trim().lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("```"));
    let Some(line) = lines.next() else {
        return Err("claude returned an empty label".into());
    };
    if lines.next().is_some() {
        return Err(format!("claude replied with prose, not a label: {}", snippet(text)));
    }
    let label = line.trim_matches(|c| matches!(c, '"' | '\'' | '`')).trim();
    let label = label.trim_end_matches(|c: char| !c.is_alphanumeric()).trim();
    if label.is_empty() {
        return Err("claude returned an empty label".into());
    }
    if label.chars().count() > 60 {
        return Err(format!("claude's label is too long: {}", snippet(label)));
    }
    Ok(label.to_string())
}

/// Ask Claude (haiku), running in the repo checkout for context, to compress an
/// existing issue's title + body into a short session label. The spawn preview
/// opens immediately with the heuristic label and swaps this in when it
/// arrives; any error here just leaves the heuristic in place.
async fn suggest_short_label(checkout: &Path, title: &str, body: &str) -> Result<String, String> {
    // Issue bodies can be arbitrarily long; the label only needs the gist.
    let body: String = body.chars().take(4000).collect();
    let prompt = format!(
        "Summarize this GitHub issue as a short workspace label. Reply with ONLY \
         the label, nothing else. The label is a short, human-readable session \
         label — plain words with normal spaces and capitalization (NOT a slug or \
         branch name, so no dashes/underscores), 2-4 words, at most 40 characters, \
         no issue number, no trailing punctuation. Prefer concrete keywords from \
         the issue (component names, actions) over a generic rephrasing.\n\n\
         Issue title: {title}\n\nIssue body:\n{body}"
    );
    let reply = claude_text(checkout, &prompt, "summarizing the issue").await?;
    parse_short_label(&reply)
}

/// Suggest an AI short label for an existing issue (title + body via Claude).
/// Called fire-and-forget by the spawn preview after it opens with the
/// heuristic label; the frontend swallows errors, so failures here are benign.
#[tauri::command]
pub async fn suggest_short_title(repo: String, issue_number: u64) -> Result<String, String> {
    crate::log_invoke!("suggest_short_title", repo = %repo, issue = issue_number);
    let settings = crate::repo_settings::repo_settings_get(repo.clone())?;
    let identity_id = settings
        .identity_id
        .ok_or_else(|| "No identity assigned to this repo. Set one in Settings → Repo.".to_string())?;
    let checkout = expand_tilde(settings.checkout_dir.as_deref().unwrap_or_default());
    if !checkout.join(".git").exists() {
        return Err(format!("checkout dir is not a git repo: {}", checkout.display()));
    }
    let gh = GitHub::for_identity(&identity_id)?;
    let (issue_title, _issue_url, issue_body) = issue_facts(&gh, &repo, issue_number).await?;
    suggest_short_label(&checkout, &issue_title, &issue_body).await
}

/// A drafted issue ready to create, or a signal that Claude couldn't produce a
/// clear draft and the user must confirm creating from raw text.
enum DraftStep {
    Ready {
        title: String,
        body: String,
        /// Punchy branch/session label produced in the same draft call.
        short_title: String,
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

    let settings = crate::repo_settings::repo_settings_get(repo.to_string())?;
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
        let title = trim_to_word(idea, 70);
        let short_title = default_short_title(&title);
        DraftStep::Ready {
            title,
            body: idea.to_string(),
            short_title,
            warning: Some("created from your text without an AI draft".to_string()),
        }
    } else {
        match draft_issue(&checkout, idea).await {
            Ok((title, body, short_title)) => DraftStep::Ready { title, body, short_title, warning: None },
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
    crate::log_invoke!("create_issue", repo = %repo, use_raw_fallback);
    let (gh, step) = resolve_draft(&repo, &idea, use_raw_fallback).await?;
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
    let settings = crate::repo_settings::repo_settings_get(repo.clone())?;
    let identity_id = settings
        .identity_id
        .ok_or_else(|| "No identity assigned to this repo. Set one in Settings → Repo.".to_string())?;
    let gh = GitHub::for_identity(&identity_id)?;
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
    repo: String,
    idea: String,
    use_raw_fallback: bool,
    force_new: bool,
) -> Result<CreateAndSpawnOutcome, String> {
    crate::log_invoke!("create_issue_and_spawn", repo = %repo, use_raw_fallback, force_new);
    let (gh, step) = resolve_draft(&repo, &idea, use_raw_fallback).await?;
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
}

/// Prepare a preview for spawning an existing issue: fetch its title/body and
/// pick theming, without touching the worktree or GitHub.
#[tauri::command]
pub async fn prepare_spawn(repo: String, issue_number: u64) -> Result<SpawnPlan, String> {
    crate::log_invoke!("prepare_spawn", repo = %repo, issue = issue_number);
    let settings = crate::repo_settings::repo_settings_get(repo.clone())?;
    let identity_id = settings
        .identity_id
        .ok_or_else(|| "No identity assigned to this repo. Set one in Settings → Repo.".to_string())?;
    let gh = GitHub::for_identity(&identity_id)?;
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
    repo: String,
    idea: String,
    use_raw_fallback: bool,
) -> Result<DraftPreviewOutcome, String> {
    crate::log_invoke!("draft_spawn_preview", repo = %repo, use_raw_fallback);
    let (_gh, step) = resolve_draft(&repo, &idea, use_raw_fallback).await?;
    match step {
        DraftStep::Ready { title, body, short_title, .. } => {
            let seed = format!("new-{}", slugify(&short_title, 25));
            let (color, emoji) = pick_theme(&seed);
            let repo_name = repo.split('/').next_back().unwrap_or(&repo).to_string();
            Ok(DraftPreviewOutcome::Drafted(SpawnPlan {
                repo,
                issue_number: None,
                issue_title: title,
                issue_body: body,
                short_title,
                color: color.to_string(),
                emoji: emoji.to_string(),
                repo_name,
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
    let settings = crate::repo_settings::repo_settings_get(repo.clone())?;
    let identity_id = settings
        .identity_id
        .ok_or_else(|| "No identity assigned to this repo. Set one in Settings → Repo.".to_string())?;
    let gh = GitHub::for_identity(&identity_id)?;

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
/// Triggered only by an explicit user click — we never launch it automatically.
#[tauri::command]
pub fn open_accessibility_settings() {
    crate::log_invoke!("open_accessibility_settings");
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
    let settings = crate::repo_settings::repo_settings_get(session.repo.clone())?;
    if let Some(identity_id) = settings.identity_id {
        if let Ok(gh) = GitHub::for_identity(&identity_id) {
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
                if let Ok(sha) = git(&work_dir, &["rev-parse", "HEAD"]) {
                    if let Ok(prs) = gh.pulls_for_commit(&session.repo, &sha).await {
                        pr_merged = prs.iter().any(|p| p["merged_at"].is_string());
                    }
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
                        if worktree_in_use(&work_dir) {
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
    git(&checkout, &["worktree", "remove", "--force", &work_dir.to_string_lossy()])?;

    // 3. Delete the local branch (-D: spawn unset the upstream and -d checks the
    //    wrong base, so it would refuse even for merged branches).
    if local_branch_exists(&checkout, &branch) {
        if let Err(e) = git(&checkout, &["branch", "-D", &branch]) {
            tracing::warn!(branch = %branch, error = %e, "could not delete local branch during teardown");
        }
    }

    // 4. Remove the leftover wrapper dir (`<prefix><workspace>`), gating removal
    //    on the path actually starting with the repo's configured worktree
    //    prefix so we never remove_dir_all something outside it.
    if let Some(parent) = work_dir.parent() {
        let prefix = settings.worktree_prefix.as_deref().unwrap_or("~/src/work-");
        let expanded = expand_tilde(prefix);
        let under_prefix = parent.to_string_lossy().starts_with(&*expanded.to_string_lossy());
        if under_prefix && parent.exists() {
            if let Err(e) = std::fs::remove_dir_all(parent) {
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
#[tracing::instrument(skip_all, fields(session = %session_id))]
pub async fn session_pr(session_id: String) -> Result<Option<PrLink>, String> {
    crate::log_invoke_debug!("session_pr");
    let Some(session) = crate::sessions::get(&session_id) else {
        return Ok(None);
    };
    let settings = crate::repo_settings::repo_settings_get(session.repo.clone())?;
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
#[tracing::instrument(skip_all, fields(session = %session_id))]
pub async fn session_create_pr(session_id: String) -> Result<PrLink, String> {
    crate::log_invoke!("session_create_pr");
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

    let settings = crate::repo_settings::repo_settings_get(session.repo.clone())?;
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

    // Seed the draft with the issue title/body for context (best-effort).
    let issue = gh.issue(&session.repo, session.issue_number).await.unwrap_or_default();
    let issue_title = issue["title"].as_str().unwrap_or("");
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

    // Draft via Claude. If drafting fails (claude errored, or its reply had no
    // parseable {title, body}), log and propagate the error and abort *before*
    // pushing or opening the PR — we'd rather tell the user why than open a
    // garbage PR. The PR draft reuses the issue-draft parser but only needs
    // title + body.
    let reply = claude_text(&work_dir, &prompt, "drafting the PR")
        .await
        .map_err(|e| { tracing::warn!(error = %e, "Claude PR draft failed"); e })?;
    let (title, body, _) = parse_issue_draft(&reply)
        .map_err(|e| { tracing::warn!(error = %e, "PR draft reply was unparseable"); e })?;

    let body = format!("{body}\n\nCloses #{}", session.issue_number);

    // Draft succeeded — now push the branch so GitHub can see the head ref. -u
    // sets upstream for the user's later pushes from the session.
    git(&work_dir, &["push", "-u", "origin", &branch])
        .map_err(|e| format!("could not push branch {branch}: {e}"))?;

    let pr = gh
        .create_pull(&session.repo, &title, &branch, &base, &body, true)
        .await?;
    let link = pr_link_from(&pr);
    tracing::info!(repo = %session.repo, branch = %branch, pr = link.number, "created pull request");
    Ok(link)
}

// ── PR checks & merge ─────────────────────────────────────────────────────────

/// Aggregate CI/merge state of a session's PR, surfaced to the UI to drive the
/// pill's check indicator and gate auto-merge.
#[derive(serde::Serialize)]
pub struct PrChecks {
    /// Pill indicator derived from `mergeable_state`: "passed" (mergeable),
    /// "failed" (conflicts), "pending" (behind/blocked/computing), or "none"
    /// (draft). See `merge_pill_state`.
    pub state: String,
    /// Whether mergeability is still being computed by GitHub (`mergeable_state`
    /// == "unknown") — drives the spinner.
    pub running: bool,
    /// GitHub's `mergeable_state == "clean"`: required checks and (where branch
    /// protection requires them) approvals are satisfied, so a merge will land.
    pub ready_to_merge: bool,
    /// Raw GitHub `mergeable_state` (clean / dirty / behind / blocked / unstable
    /// / draft / unknown). Lets the UI stop the auto-merge loop on states that
    /// won't self-resolve (behind, conflicts, required review) instead of waiting.
    pub mergeable_state: String,
}

/// Map GitHub's `mergeable_state` to the pill's indicator vocabulary
/// (`passed`/`failed`/`pending`/`none`) plus whether to show the computing
/// spinner. We can't read the Checks API with a fine-grained token, so
/// merge-readiness — which only needs "Pull requests: read" — is the signal the
/// pill reflects. `mergeable_state` still folds in required-check results
/// (`blocked`/`unstable`) without us touching the Checks API.
fn merge_pill_state(mergeable_state: &str) -> (String, bool) {
    match mergeable_state {
        // Mergeable: clean is fully green; unstable/has_hooks are mergeable too
        // (a non-required check may be red, but the merge will land).
        "clean" | "unstable" | "has_hooks" => ("passed".to_string(), false),
        "dirty" => ("failed".to_string(), false), // conflicts with base
        "behind" | "blocked" => ("pending".to_string(), false), // needs update / review
        "draft" => ("none".to_string(), false),
        // "unknown" / "" — GitHub is still computing mergeability; spin.
        _ => ("pending".to_string(), true),
    }
}

/// Check status for a session's open PR, if any. Mirrors `session_pr`'s soft-fail
/// contract: `Ok(None)` when the session is gone, the repo has no identity, or
/// the branch has no open PR. Only an actual API failure surfaces as `Err`, which
/// the frontend also degrades silently (no indicator).
#[tauri::command]
pub async fn session_pr_checks(session_id: String) -> Result<Option<PrChecks>, String> {
    let Some(session) = crate::sessions::get(&session_id) else {
        return Ok(None);
    };
    let settings = crate::repo_settings::repo_settings_get(session.repo.clone())?;
    let Some(identity_id) = settings.identity_id else {
        return Ok(None);
    };
    let gh = GitHub::for_identity(&identity_id)?;
    let prs = gh.pulls_for_branch(&session.repo, &session.branch).await?;

    let created_at = |p: &&serde_json::Value| p["created_at"].as_str().unwrap_or("").to_string();
    let Some(pr) = prs
        .iter()
        .filter(|p| p["state"].as_str() == Some("open"))
        .max_by_key(created_at)
    else {
        return Ok(None);
    };
    let Some(number) = pr["number"].as_u64() else {
        return Ok(None);
    };

    // `mergeable_state` is only populated on the single-PR endpoint, not the
    // list. It needs just "Pull requests: read" — no Checks API (which a
    // fine-grained PAT can't access) — and it already reflects required-check
    // results via `blocked`/`unstable`.
    let full = gh.pull(&session.repo, number).await?;
    let mergeable_state = full["mergeable_state"].as_str().unwrap_or("unknown").to_string();
    let ready_to_merge = mergeable_state == "clean";
    let (state, running) = merge_pill_state(&mergeable_state);

    Ok(Some(PrChecks { state, running, ready_to_merge, mergeable_state }))
}

// ── Work lifecycle (local git facts) ────────────────────────────────────────────

/// Local git facts about a session's worktree, used by the UI to derive the
/// work-lifecycle phase (Planning → Implementing → Merged). The "Merged" phase
/// comes from the PR state the row already has, so this stays purely local —
/// no GitHub round-trip on a polled command.
#[derive(serde::Serialize)]
pub struct WorkState {
    /// Commits in `origin/<default_branch>..HEAD` (measured against the local
    /// remote-tracking ref — no fetch, so it can lag origin slightly).
    pub ahead: u32,
    /// Whether the worktree has uncommitted changes (`git status --porcelain`).
    pub dirty: bool,
}

/// Local git state of a session's worktree. Soft-fails to `Ok(None)` when the
/// session record or its worktree is gone (e.g. torn down mid-poll), matching
/// `session_pr` / `session_pr_checks`; the UI then renders no lifecycle pill.
/// Deliberately local-only (no `git fetch`) to stay cheap on the poll path.
#[tauri::command]
pub async fn session_work_state(session_id: String) -> Result<Option<WorkState>, String> {
    crate::log_invoke_debug!("session_work_state");
    let Some(session) = crate::sessions::get(&session_id) else {
        return Ok(None);
    };
    let work_dir = PathBuf::from(&session.work_dir);
    if !work_dir.exists() {
        return Ok(None);
    }
    let base = session.default_branch;

    let dirty = git(&work_dir, &["status", "--porcelain"])
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let ahead = git(&work_dir, &["rev-list", "--count", &format!("origin/{base}..HEAD")])
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);

    Ok(Some(WorkState { ahead, dirty }))
}

/// Merge a session's PR, creating it first if needed. Before touching GitHub it
/// reconciles the **local worktree** so the merge can't land a stale remote: it
/// blocks on uncommitted changes and pushes any committed-but-unpushed local
/// commits (so the PR head reflects local HEAD). Then it reuses/creates the PR,
/// marks a draft ready for review (a draft can't be merged and its checks don't
/// gate), and merges — but only once GitHub reports the PR `clean`.
///
/// States that won't self-resolve surface as `Err` so the UI can stop waiting:
/// `dirty` (conflicts) and `behind` (out of date with base). Transient states
/// (checks still running, GitHub recomputing) return the PR unmerged so the
/// frontend poll retries.
#[tauri::command]
pub async fn session_merge_pr(session_id: String) -> Result<PrLink, String> {
    let session = crate::sessions::get(&session_id)
        .ok_or_else(|| format!("session not found: {session_id}"))?;
    let work_dir = PathBuf::from(&session.work_dir);
    let branch = session.branch.clone();
    let settings = crate::repo_settings::repo_settings_get(session.repo.clone())?;
    let identity_id = settings
        .identity_id
        .ok_or_else(|| "No identity assigned to this repo. Set one in Settings → Repo.".to_string())?;
    let gh = GitHub::for_identity(&identity_id)?;

    // Guard: uncommitted work would be silently excluded — the merge lands the
    // pushed branch, not the worktree. Block and ask the user to commit first.
    let dirty = git(&work_dir, &["status", "--porcelain"])
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    if dirty {
        return Err("This worktree has uncommitted changes. Commit them first, then merge.".to_string());
    }

    // Reconcile committed-but-unpushed local commits before merging: refresh the
    // remote ref, and if local HEAD is ahead, push so the PR head includes them.
    git(&work_dir, &["fetch", "origin", &branch, "--quiet"]).ok();
    let ahead = git(&work_dir, &["rev-list", "--count", &format!("origin/{branch}..HEAD")])
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);
    if ahead > 0 {
        git(&work_dir, &["push", "origin", &branch])
            .map_err(|e| format!("could not push local commits before merging: {e}"))?;
    }

    // Ensure a PR exists: reuse an open one, else create (which drafts + pushes).
    let prs = gh.pulls_for_branch(&session.repo, &branch).await?;
    let number = match prs.iter().find(|p| p["state"].as_str() == Some("open")) {
        Some(p) => p["number"].as_u64().unwrap_or(0),
        None => session_create_pr(session_id.clone()).await?.number,
    };

    // Re-fetch so `draft` / `mergeable_state` reflect the (possibly just-created)
    // PR and any commits we just pushed.
    let mut pr = gh.pull(&session.repo, number).await?;

    // A draft can't be merged and its checks don't gate; promote it first.
    if pr["draft"].as_bool().unwrap_or(false) {
        let node_id = pr["node_id"].as_str().unwrap_or("").to_string();
        if node_id.is_empty() {
            return Err("could not resolve PR node id to mark it ready for review".to_string());
        }
        gh.mark_ready(&node_id).await?;
        pr = gh.pull(&session.repo, number).await?;
    }

    match pr["mergeable_state"].as_str().unwrap_or("") {
        "clean" => {
            gh.merge_pull(&session.repo, number, "merge").await?;
            let merged = gh.pull(&session.repo, number).await?;
            Ok(pr_link_from(&merged))
        }
        "dirty" => Err(format!(
            "PR #{number} has merge conflicts with {base}. Resolve them in the worktree and push, then merge.",
            base = session.default_branch,
        )),
        "behind" => Err(format!(
            "PR #{number} is behind {base}. Update the branch (merge or rebase {base}) and push, then merge.",
            base = session.default_branch,
        )),
        // blocked / unstable / unknown (null while GitHub recomputes): transient
        // or gated on checks/review — return the PR as-is so the poll keeps
        // waiting; the frontend decides when a blocker is terminal.
        _ => Ok(pr_link_from(&pr)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Re-merging with a new binary rewrites only *our* hooks for this workspace,
    /// leaving an unrelated user hook and another workspace's hook untouched.
    #[test]
    fn merge_replaces_only_our_hooks_for_this_ws() {
        let ws = "35-status-hooks-fix";
        let old_bin = Path::new("/old/work-8/backend/target/debug/maiestro");

        // A settings file as a prior spawn wrote it, plus an unrelated user hook
        // and a *different* workspace's status hook sharing the Stop event.
        let mut root = merge_hooks(serde_json::json!({}), old_bin, ws);
        let stop = root["hooks"]["Stop"].as_array_mut().unwrap();
        stop.push(serde_json::json!({ "hooks": [{ "type": "command", "command": "echo hi" }] }));
        stop.push(serde_json::json!({
            "hooks": [{ "type": "command", "command": "'/x/maiestro' hook idle --workspace '99-other'" }]
        }));

        let new_bin = Path::new("/Applications/mAIestro.app/Contents/MacOS/maiestro");
        let merged = merge_hooks(root, new_bin, ws);

        let cmds: Vec<&str> = merged["hooks"]["Stop"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|g| g["hooks"][0]["command"].as_str())
            .collect();

        // Our stale entry was rewritten to the new binary; the old path is gone.
        assert!(cmds.iter().any(|c| c.contains("/Applications/mAIestro.app") && c.contains("--workspace '35-status-hooks-fix'")));
        assert!(!cmds.iter().any(|c| c.contains("/old/work-8")));
        // Exactly one of our entries for this ws remains (no duplication).
        assert_eq!(cmds.iter().filter(|c| is_maiestro_hook(c, ws)).count(), 1);
        // Unrelated and other-workspace hooks survive untouched.
        assert!(cmds.contains(&"echo hi"));
        assert!(cmds.iter().any(|c| c.contains("--workspace '99-other'")));
    }

    /// A worktree carrying our hooks is detected; an unrelated-only file is not.
    #[test]
    fn detects_our_hooks() {
        let ws = "12-foo";
        let ours = merge_hooks(serde_json::json!({}), Path::new("/bin/maiestro"), ws);
        assert!(has_maiestro_hooks(&ours, ws));

        let unrelated = serde_json::json!({
            "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "echo hi" }] }] }
        });
        assert!(!has_maiestro_hooks(&unrelated, ws));
        // Our hooks for a *different* ws don't count as this ws's.
        assert!(!has_maiestro_hooks(&ours, "99-other"));
    }

    /// A bare label parses; quote/backtick/fence wrapping and trailing
    /// punctuation are stripped.
    #[test]
    fn short_label_parses_and_unwraps() {
        assert_eq!(parse_short_label("Auth token refresh").unwrap(), "Auth token refresh");
        assert_eq!(parse_short_label("  \"Auth token refresh.\"  ").unwrap(), "Auth token refresh");
        assert_eq!(parse_short_label("`Resizable popover`").unwrap(), "Resizable popover");
        assert_eq!(parse_short_label("```\nAI workspace labels\n```").unwrap(), "AI workspace labels");
    }

    /// Empty, multi-line (prose), and over-long replies are rejected — the
    /// caller falls back to the heuristic label.
    #[test]
    fn short_label_rejects_prose() {
        assert!(parse_short_label("").is_err());
        assert!(parse_short_label("\"\"").is_err());
        assert!(parse_short_label("Here are some options:\n- Auth refresh\n- Token renewal").is_err());
        assert!(parse_short_label(&"long ".repeat(20)).is_err());
    }
}
