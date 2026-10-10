//! Where a session runs: its **host**, the user-facing window the agent's
//! terminal lives in — a VS Code window (`editor.rs`) or a Terminal.app window
//! (`terminal.rs`). mAIestro Code launches the session into the host and never
//! hosts it itself (see CLAUDE.md).
//!
//! A repo chooses the host for new sessions (`session_host`); each session
//! records the host it was spawned with, like its agent, so changing the repo
//! setting never moves an existing worktree. Everything that opens, focuses,
//! probes or closes a session's window goes through the dispatch here, so the
//! spawn, reopen, agent-switch and teardown paths stay host-agnostic.
//!
//! The session command line itself ([`session_argv`]) lives here too: VS Code's
//! folder-open task and a terminal host run exactly the same resolved-agent
//! command.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agent::Agent;
use crate::editor::WindowClose;
use crate::sessions::Session;
use crate::tools::shell_quote;

/// The app a repo opens its sessions in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionHost {
    /// A VS Code window whose folder-open task starts the agent.
    #[default]
    Vscode,
    /// A Terminal.app window (macOS only).
    TerminalApp,
}

impl SessionHost {
    /// Whether this host exists on the OS mAIestro Code was built for.
    pub fn supported_here(self) -> bool {
        match self {
            SessionHost::Vscode => true,
            SessionHost::TerminalApp => cfg!(target_os = "macos"),
        }
    }

    /// The app's name, for user-facing messages.
    pub fn app_name(self) -> &'static str {
        match self {
            SessionHost::Vscode => "Visual Studio Code",
            SessionHost::TerminalApp => "Terminal",
        }
    }

    /// The macOS grant mAIestro Code needs to see and close this host's
    /// windows: Accessibility for VS Code (System Events), Automation for
    /// Terminal (its own Apple events).
    pub fn permission(self) -> Permission {
        match self {
            SessionHost::Vscode => Permission::Accessibility,
            SessionHost::TerminalApp => Permission::Automation,
        }
    }
}

impl std::fmt::Display for SessionHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SessionHost::Vscode => "vscode",
            SessionHost::TerminalApp => "terminal_app",
        })
    }
}

/// A macOS privacy grant the UI can open System Settings at, when its absence
/// is what blocked closing a session's window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Privacy & Security → Accessibility (System Events window control).
    Accessibility,
    /// Privacy & Security → Automation (Apple events to Terminal).
    Automation,
}

impl Permission {
    /// How to ask the user for the grant, completing "…, or <this> so it can
    /// close the window for you."
    pub fn grant_phrase(self) -> &'static str {
        match self {
            Permission::Accessibility => "enable Accessibility for mAIestro Code",
            Permission::Automation => "allow mAIestro Code to control Terminal (Automation)",
        }
    }
}

// ── The session command ─────────────────────────────────────────────────────────

/// Per-repo launch preferences for the session command, read from the repo's
/// **current** settings each time the task is written (spawn, reopen, agent
/// switch) — unlike the color and agent, which come from the session record,
/// these are preferences rather than part of the worktree's identity. See
/// [`session_argv`] for which agent honors which.
#[derive(Debug, Clone, Copy)]
pub struct LaunchOptions {
    /// Claude: pass `--remote-control` (`agent_settings.claude.remote_control`).
    pub remote_control: bool,
}

impl LaunchOptions {
    /// The launch options `settings` asks for.
    pub fn from_settings(settings: &crate::repo_settings::RepoSettings) -> Self {
        Self { remote_control: crate::repo_settings::claude_remote_control(settings) }
    }
}

/// The program and arguments that start a real, user-facing `agent` session:
/// what VS Code's folder-open task runs in its integrated terminal, and what a
/// terminal host types into its new window. Each argument carries whether the
/// POSIX shell form ([`shell_command`], macOS) quotes it; VS Code on Windows
/// runs the argv directly as a `process` task.
///
/// **Claude:** `--remote-control` (unless the repo turned it off, see
/// [`LaunchOptions`]) lets the user drive the session remotely; mAIestro Code
/// still only launches it, it does not host it. `--name` gives the
/// session the same display name mAIestro Code tracks it by, and the trailing
/// `/color <name>` prompt carries the worktree's theme into the session UI so it
/// matches the dashboard row and the title bar. The color goes through the
/// initial *prompt* rather than a flag because Claude Code's `--agent-color` is
/// only honored alongside `--agent-id`/`--agent-name`/`--team-name` (teammate
/// sessions); passing it on its own is silently ignored. A leading-slash initial
/// prompt is dispatched as a command, so it costs one line in the transcript and
/// no model call.
///
/// **Codex:** the binary plus mAIestro Code's status hooks as `-c hooks.…`
/// session flags (`hooks::codex_hook_overrides`) — identical for every worktree,
/// so the user's one-time Codex hook trust covers them all. No initial prompt:
/// Codex has no `--name` or `/color`, and any prompt would start a real model
/// turn, so the session carries no name or color of its own — the host window
/// is still themed.
///
/// **Antigravity:** the bare binary. Its status hooks live in the worktree's
/// `.agents/hooks.json` (`hooks/antigravity.rs`), and it has no session-name flag or
/// `/color`; an initial prompt (`-i`) would start a real model turn. Antigravity
/// asks the user to trust each new worktree folder at startup — its own prompt.
///
/// **Copilot:** the binary plus `--name <session title>`, as Claude gets. Its
/// status hooks live in the worktree's `.github/hooks/maiestro-status.json`
/// (`hooks/copilot.rs`), loaded once the user answers Copilot's own "Do you trust the
/// files in this folder?" prompt. No initial prompt: `-i` would start a real
/// model turn (a premium request), and Copilot has no `/color` anyway. No
/// `--no-auto-update` either — updating the interactive CLI is the user's call.
///
/// Whatever the agent, the binary is the **resolved** agent path (`tools::resolve_tool`),
/// not a bare name left to PATH. In VS Code the task runs in the integrated
/// terminal, whose PATH is whatever the VS Code process inherited —
/// and when mAIestro Code launched that VS Code from the packaged bundle at login,
/// that can be the minimal Launch Services PATH with no agent on it. The same
/// `tool_paths` override that pins mAIestro Code's own drafting calls therefore
/// also decides which binary the session starts with. When nothing concrete
/// resolves, `resolve_tool` yields the bare name, i.e. exactly the previous
/// behavior.
pub(crate) fn session_argv(agent: Agent, color: &str, session_title: &str, launch: LaunchOptions) -> (String, Vec<(String, bool)>) {
    let bin = crate::tools::resolve_tool(agent.tool()).to_string_lossy().into_owned();
    let flag = |s: &str| (s.to_string(), false);
    let value = |s: String| (s, true);
    let args = match agent {
        Agent::Claude => launch
            .remote_control
            .then(|| flag("--remote-control"))
            .into_iter()
            .chain([
                flag("--name"),
                value(session_title.to_string()),
                value(format!("/color {}", crate::theming::claude_color(color))),
            ])
            .collect(),
        Agent::Codex => crate::hooks::codex_hook_overrides()
            .into_iter()
            .flat_map(|o| [flag("-c"), value(o)])
            .collect(),
        Agent::Antigravity => Vec::new(),
        Agent::Copilot => vec![flag("--name"), value(session_title.to_string())],
    };
    (bin, args)
}

/// The POSIX shell form of [`session_argv`]: the program and every value
/// argument single-quoted, flags bare.
pub(crate) fn shell_command(program: &str, args: &[(String, bool)]) -> String {
    std::iter::once(shell_quote(program))
        .chain(args.iter().map(|(a, quote)| if *quote { shell_quote(a) } else { a.clone() }))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The launch options for a session of `repo`, from the repo's *current*
/// settings. A settings file that can't be read warns and falls back to the
/// schema defaults: the launch itself must still go ahead.
pub fn launch_options(repo: &str) -> LaunchOptions {
    match crate::repo_settings::repo_settings_get(repo.to_string()) {
        Ok(settings) => LaunchOptions::from_settings(&settings),
        Err(e) => {
            tracing::warn!(error = %e, "could not read repo settings; launching with the default options");
            LaunchOptions::from_settings(&crate::repo_settings::RepoSettings::default_for(repo))
        }
    }
}

/// The title a terminal host gives the session's window: `<repo>: <session title>`.
fn terminal_title(session: &Session) -> String {
    let repo_name = session.repo.split('/').next_back().unwrap_or(&session.repo);
    format!("{repo_name}: {}", session.session_title)
}

// ── Dispatch ────────────────────────────────────────────────────────────────────

/// Start the session in a fresh window of its host. For Terminal this records
/// the new window's handle on the session, so focus and teardown can find it.
pub async fn open(session: &Session) -> Result<(), String> {
    let work_dir = PathBuf::from(&session.work_dir);
    match session.host {
        SessionHost::Vscode => crate::editor::open_vscode(&work_dir),
        SessionHost::TerminalApp => {
            if !SessionHost::TerminalApp.supported_here() {
                return Err("Terminal.app sessions are only available on macOS".into());
            }
            let (program, args) =
                session_argv(session.agent, &session.color, &session.session_title, launch_options(&session.repo));
            let handle = crate::terminal::launch(
                &work_dir,
                &shell_command(&program, &args),
                &terminal_title(session),
                &session.color,
            )
            .await?;
            tracing::info!(window_id = handle.window_id, tty = %handle.tty, "opened the session in Terminal");
            // Re-read the record: it may have changed while the launch waited on
            // Terminal (or on the user answering its Automation prompt).
            let mut current = crate::sessions::get(&session.id).unwrap_or_else(|| session.clone());
            current.terminal_window = Some(handle);
            crate::sessions::save(&current).map_err(|e| format!("could not record the Terminal window: {e}"))
        }
    }
}

/// Bring the session up when its worktree is reopened. VS Code is simply asked
/// to open the folder again (it brings an existing window forward itself);
/// Terminal focuses the session's window and opens a new one only when that
/// window is gone — never a second window next to a live one, which would run
/// a second agent in the same worktree.
pub async fn reopen(session: &Session) -> Result<(), String> {
    match session.host {
        SessionHost::Vscode => crate::editor::open_vscode(Path::new(&session.work_dir)),
        SessionHost::TerminalApp => focus_or_open(session).await,
    }
}

/// Focus the session's window if it is open, otherwise open one.
pub async fn focus_or_open(session: &Session) -> Result<(), String> {
    match session.host {
        SessionHost::Vscode => crate::editor::focus_or_open(Path::new(&session.work_dir)).await,
        SessionHost::TerminalApp => {
            if let Some(handle) = &session.terminal_window {
                // A probe we can't run (no Automation grant) is an error, not
                // "absent": launching would fail the same way, and if it didn't
                // it would start a second agent next to the live one.
                if crate::terminal::focus(handle).await? {
                    return Ok(());
                }
            }
            open(session).await
        }
    }
}

/// Whether the session's window is (or, when we can't look, appears to be)
/// open. Without the grant to look, falls back to the permission-free check
/// that something is running inside the worktree.
pub async fn window_open(session: &Session) -> bool {
    let work_dir = PathBuf::from(&session.work_dir);
    match session.host {
        SessionHost::Vscode => crate::editor::editor_window_open(&work_dir).await,
        SessionHost::TerminalApp => match &session.terminal_window {
            None => false,
            Some(handle) => match crate::terminal::probe(handle).await {
                Ok(open) => open,
                Err(e) => {
                    tracing::warn!(error = %e, "could not probe the Terminal window; checking the worktree instead");
                    crate::editor::worktree_in_use(&work_dir).await
                }
            },
        },
    }
}

/// Close the session's window and confirm it is gone (see
/// [`crate::editor::close_window_and_wait`] for why confirmation matters).
pub async fn close_and_wait(session: &Session) -> WindowClose {
    let work_dir = PathBuf::from(&session.work_dir);
    match session.host {
        SessionHost::Vscode => crate::editor::close_window_and_wait(&work_dir).await,
        SessionHost::TerminalApp => match &session.terminal_window {
            None => WindowClose::Closed,
            Some(handle) => crate::terminal::close_and_wait(handle, &work_dir).await,
        },
    }
}

/// Why a session's window couldn't be closed, worded for `action` ("tear
/// down", "restart the session"), and the grant that would let mAIestro Code
/// close it itself (`None` when the grant is there and the close just didn't
/// take). `None` overall when the window is closed.
pub fn blocked(host: SessionHost, close: WindowClose, action: &str) -> Option<(String, Option<Permission>)> {
    let app = host.app_name();
    match close {
        WindowClose::Closed => None,
        WindowClose::InUse => Some((
            format!(
                "I couldn't {action} because the {app} window is still open.\n\nYou have two options: \
                 close the window yourself, or {} so it can close the window for you.",
                host.permission().grant_phrase()
            ),
            Some(host.permission()),
        )),
        WindowClose::StillOpen => Some((
            format!("I couldn't {action} because the {app} window is still open. Close its window, then try again."),
            None,
        )),
    }
}

/// Open System Settings → Privacy & Security → Automation, where the user lets
/// mAIestro Code control Terminal. Triggered only by an explicit user click.
/// macOS only, like `editor::open_accessibility_settings`.
#[tauri::command]
pub fn open_automation_settings() {
    crate::log_invoke!("open_automation_settings");
    #[cfg(not(target_os = "windows"))]
    let _ = crate::tools::spawn_reaped(
        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Automation"),
    );
    #[cfg(target_os = "windows")]
    tracing::warn!("open_automation_settings called on Windows, which has no Automation grant");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_values_match_the_settings_schema() {
        assert_eq!(serde_json::to_value(SessionHost::Vscode).unwrap(), "vscode");
        assert_eq!(serde_json::to_value(SessionHost::TerminalApp).unwrap(), "terminal_app");
        assert_eq!(SessionHost::TerminalApp.to_string(), "terminal_app");
        let schema = crate::repo_settings::repo_settings_schema();
        let values: Vec<String> = schema["properties"]["session_host"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        assert_eq!(values, ["vscode", "terminal_app"]);
    }

    #[test]
    fn terminal_title_is_repo_then_session_title() {
        let s: Session = serde_json::from_value(serde_json::json!({
            "id": "243-x", "repo": "acme/widgets", "issue_number": 243, "issue_url": "u", "branch": "b",
            "work_dir": "/w", "cloned_repo_dir": "/c", "session_title": "🍋 #243 — Terminal host", "color": "#ca8a04", "emoji": "🍋",
        }))
        .unwrap();
        assert_eq!(terminal_title(&s), "widgets: 🍋 #243 — Terminal host");
        assert_eq!(s.host, SessionHost::Vscode, "records without a host load as VS Code");
    }

    #[test]
    fn blocked_names_the_host_and_its_grant() {
        assert!(blocked(SessionHost::Vscode, WindowClose::Closed, "tear down").is_none());
        let (msg, perm) = blocked(SessionHost::TerminalApp, WindowClose::InUse, "tear down").unwrap();
        assert!(msg.contains("Terminal window"), "{msg}");
        assert!(msg.contains("Automation"), "{msg}");
        assert_eq!(perm, Some(Permission::Automation));
        let (msg, perm) = blocked(SessionHost::Vscode, WindowClose::InUse, "tear down").unwrap();
        assert!(msg.contains("Visual Studio Code window") && msg.contains("Accessibility"), "{msg}");
        assert_eq!(perm, Some(Permission::Accessibility));
        let (_, perm) = blocked(SessionHost::Vscode, WindowClose::StillOpen, "tear down").unwrap();
        assert_eq!(perm, None);
    }
}
