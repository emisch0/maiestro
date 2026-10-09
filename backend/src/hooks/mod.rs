//! Agent hooks (Claude Code, Codex, Antigravity, or Copilot) that give mAIestro Code live
//! per-session status.
//!
//! At worktree creation the spawn path installs the session agent's status hooks
//! ([`write_session_hooks`]). Each hook invokes *this* binary as `maiestro hook
//! <state> --workspace <ws-id>` (or, for Codex, a stable wrapper around it),
//! which writes a status record the backend watches. See `status.rs` for the
//! record side and CLAUDE.md ("Live per-session status via agent hooks") for
//! the full design.
//!
//! Because the helper path is `current_exe()` baked in at spawn time, a
//! torn-down/rebuilt spawner leaves stale paths behind; `reconcile_session_hooks`
//! (and `reconcile_all_session_hooks` at startup) heal them to the running binary.
//!
//! This module holds what the agents share: the per-agent shell quoting
//! ([`HookShell`]), the dispatch entry points, the stable hook wrapper
//! ([`ensure_hook_wrapper`]) and the `info/exclude` handling. Each agent's own
//! hook format lives in its submodule: [`claude`], [`codex`], [`antigravity`]
//! and [`copilot`].

use std::path::{Path, PathBuf};

use crate::agent::Agent;
use crate::gitops::git;
use crate::paths::expand_tilde;

mod antigravity;
mod claude;
mod codex;
mod copilot;

use antigravity::{
    antigravity_hook_program, ensure_antigravity_gitignored, reconcile_antigravity_hooks, write_antigravity_hooks,
};
use claude::{reconcile_claude_hooks, write_claude_hooks};
use copilot::{reconcile_copilot_hooks, write_copilot_hooks, COPILOT_HOOK_REL};

pub use antigravity::remove_antigravity_hooks;
pub use claude::remove_claude_hooks;
pub(crate) use codex::codex_app_server_request;
pub use codex::codex_hook_overrides;
// A Tauri command is registered through macros `#[tauri::command]` generates
// next to it, so `generate_handler!` needs those re-exported with it.
pub use codex::{__cmd__codex_hooks_review_needed, __tauri_command_name_codex_hooks_review_needed, codex_hooks_review_needed};
pub use copilot::remove_copilot_hooks;

/// The shell an agent runs a hook `command` (or Claude's `statusLine`) with,
/// which decides how we quote and wrap it. It follows the *agent* on the
/// target OS, not the OS alone (issue #199): on Windows, Claude Code runs hooks
/// through Git Bash, while Codex (`pwsh`/`powershell.exe -NoProfile -Command`)
/// and Copilot (its `powershell` field) use PowerShell. Antigravity runs
/// `cmd /c "<command>"`, built Go-style, so every `"` in the command reaches
/// `cmd` as a literal `\"` it can't parse: its commands use no double quotes at
/// all. Hooks deliberately don't use the OS-wide `tools::shell_quote`, so a
/// change there can't silently break a hook the agent runs in a different shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HookShell {
    Posix,
    PowerShell,
    Cmd,
}

impl HookShell {
    /// The shell `agent` runs hook commands with on this OS.
    pub(crate) fn for_agent(agent: Agent) -> Self {
        match agent {
            _ if !cfg!(windows) => HookShell::Posix,
            Agent::Claude => HookShell::Posix,
            Agent::Antigravity => HookShell::Cmd,
            Agent::Codex | Agent::Copilot => HookShell::PowerShell,
        }
    }

    /// `s` as one word. POSIX single-quotes it and escapes an embedded `'` as
    /// `'\''`; PowerShell single-quotes it and doubles an embedded one (the
    /// typographic single quotes count as quotes too, so those are doubled the
    /// same way). `cmd` can't take quotes here (see [`HookShell`]), so it puts a
    /// `^` before every ASCII character that isn't plainly part of a path,
    /// which makes spaces and `& ( ) % , ; = ! ^` literal.
    fn quote(self, s: &str) -> String {
        match self {
            HookShell::Posix => format!("'{}'", s.replace('\'', r"'\''")),
            HookShell::Cmd => {
                let mut out = String::with_capacity(s.len() * 2);
                for c in s.chars() {
                    if c.is_ascii() && !c.is_ascii_alphanumeric() && !matches!(c, '\\' | '/' | ':' | '.' | '_' | '-') {
                        out.push('^');
                    }
                    out.push(c);
                }
                out
            }
            HookShell::PowerShell => {
                let mut out = String::with_capacity(s.len() + 2);
                out.push('\'');
                for c in s.chars() {
                    if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
                        out.push(c);
                    }
                    out.push(c);
                }
                out.push('\'');
                out
            }
        }
    }

    /// The start of a command that runs the program at `path`. PowerShell
    /// needs the call operator: a bare quoted string is just a value to print.
    fn program(self, path: &Path) -> String {
        let quoted = self.quote(&path.to_string_lossy());
        match self {
            HookShell::Posix | HookShell::Cmd => quoted,
            HookShell::PowerShell => format!("& {quoted}"),
        }
    }

    /// `command` made silent and always successful, even when its program is
    /// missing. PowerShell raises a missing program before any redirection
    /// applies, hence the `try`; `cmd`'s "not recognized" goes to the
    /// redirected stderr. `&` runs `exit /b 0` whatever the command did.
    fn silent(self, command: &str) -> String {
        match self {
            HookShell::Posix => format!("{command} >/dev/null 2>&1 || true"),
            HookShell::PowerShell => format!("try {{ {command} *> $null }} catch {{ }}; exit 0"),
            HookShell::Cmd => format!("{command} >nul 2>&1 & exit /b 0"),
        }
    }

    /// A command that prints `line` and a newline to stdout. Under `cmd` a `"`
    /// in `line` would reach the output as `\"` (see [`HookShell`]).
    pub(crate) fn print_line(self, line: &str) -> String {
        match self {
            HookShell::Posix => format!("printf '%s\\n' {}", self.quote(line)),
            HookShell::PowerShell => format!("Write-Output {}", self.quote(line)),
            HookShell::Cmd => format!("echo {}", self.quote(line)),
        }
    }

    /// `maiestro hook <verb> --workspace <ws_id>` against `bin`.
    fn hook_command(self, bin: &Path, verb: &str, ws_id: &str) -> String {
        format!("{} hook {verb} --workspace {}", self.program(bin), self.quote(ws_id))
    }
}

/// Install the agent's status hooks for a freshly spawned worktree. Claude: merge
/// them into [`claude::claude_hook_file`] (never overwriting user/repo settings or
/// unrelated hooks). Codex: nothing is written into the worktree — its hooks ride
/// on the launch command ([`codex_hook_overrides`]) — so only the stable wrapper
/// they call is refreshed. Antigravity: set our named group in
/// [`antigravity::antigravity_hook_file`], and make sure the worktree's `.gitignore` covers it
/// ([`ensure_antigravity_gitignored`]). Copilot: write our own
/// [`copilot::copilot_hook_file`]. Either way the generated files are
/// excluded from git. Returns a notice for the user when it changed something
/// they should know about (today: the `.gitignore`).
pub async fn write_session_hooks(work_dir: &Path, ws_id: &str, agent: Agent) -> Result<Option<String>, String> {
    let notice = match agent {
        Agent::Claude => {
            let bin = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
            write_claude_hooks(work_dir, ws_id, &bin)?;
            None
        }
        Agent::Codex => {
            ensure_hook_wrapper()?;
            None
        }
        Agent::Antigravity => {
            let bin = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
            write_antigravity_hooks(work_dir, ws_id, &antigravity_hook_program(&bin)?)?;
            ensure_antigravity_gitignored(work_dir, true)
        }
        Agent::Copilot => {
            let bin = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
            write_copilot_hooks(work_dir, ws_id, &bin)?;
            None
        }
    };
    exclude_generated_files(work_dir).await;
    Ok(notice)
}

/// Rewrite a tracked session's status hooks to point at the *currently running*
/// binary, healing a stale `current_exe()` path baked in by a spawner that has
/// since been torn down or rebuilt (issue #35). Best-effort and quiet:
///
/// - Codex: the hook definitions never change (they call the stable wrapper), so
///   this only re-points the wrapper at the running binary ([`ensure_hook_wrapper`]).
/// - Claude: no-op if the worktree or its `.claude/settings.local.json` is gone,
///   or if the file carries none of our hooks (we never inject into a worktree
///   that didn't already have them). Where our hooks are, it also adds our
///   quota status line if the file has no `statusLine` yet (so worktrees
///   spawned before issue #205 pick it up) — the one thing reconcile adds
///   rather than re-points. Writes only when the resulting JSON actually
///   changed, so it doesn't churn the file on every launch.
/// - Antigravity: the same rules for our group in `.agents/hooks.json`, plus
///   re-adding its `.gitignore` line if it went missing (which records a notice
///   on the session).
/// - Copilot: rewrite our own `.github/hooks/maiestro-status.json`, only when
///   it is already there and differs.
///
/// Returns true when it rewrote something.
pub fn reconcile_session_hooks(work_dir: &Path, ws_id: &str, agent: Agent) -> bool {
    match agent {
        Agent::Claude => std::env::current_exe().is_ok_and(|bin| reconcile_claude_hooks(work_dir, ws_id, &bin)),
        Agent::Codex => ensure_hook_wrapper().unwrap_or(false),
        Agent::Antigravity => std::env::current_exe()
            .ok()
            .and_then(|bin| antigravity_hook_program(&bin).ok())
            .is_some_and(|program| reconcile_antigravity_hooks(work_dir, ws_id, &program)),
        Agent::Copilot => std::env::current_exe().is_ok_and(|bin| reconcile_copilot_hooks(work_dir, ws_id, &bin)),
    }
}

/// At startup, heal stale hook binary paths across every tracked Claude,
/// Antigravity and Copilot session (see `reconcile_session_hooks`), and point the Codex
/// wrapper at the running binary (once, however many Codex sessions exist). Logs
/// what it rewrote.
pub fn reconcile_all_session_hooks() {
    let wrapper = matches!(ensure_hook_wrapper(), Ok(true));
    let fixed = crate::sessions::load_all()
        .into_iter()
        .filter(|s| s.agent != Agent::Codex)
        .filter(|s| reconcile_session_hooks(Path::new(&s.work_dir), &s.id, s.agent))
        .count();
    if fixed > 0 || wrapper {
        tracing::info!(sessions = fixed, codex_wrapper = wrapper, "reconciled stale status-hook paths");
    }
}

/// One Antigravity or Copilot hook command: run the helper with `verb` for
/// `ws_id`, then discard its output and exit 0 no matter what. For `PreToolUse`,
/// Antigravity treats any stdout (even `{}`) or a non-zero exit as a *deny*, and
/// empty output as "no opinion"; Copilot reads a `preToolUse` /
/// `permissionRequest` hook's output as a decision too. So this must stay silent
/// and succeed even when the baked binary no longer exists, or it would block
/// the user's tools. `shell` is the one the agent runs it with.
fn silent_hook_command(shell: HookShell, bin: &Path, ws_id: &str, verb: &str) -> String {
    shell.silent(&shell.hook_command(bin, verb, ws_id))
}

// ── The stable hook wrapper (Codex, and Antigravity on Windows) ───────────────

/// The stable path every Codex hook, and on Windows every Antigravity hook
/// ([`antigravity_hook_program`]), invokes. Its *contents* follow the running
/// binary; its *path* never changes, which is what keeps the hook definitions —
/// and so the user's one-time Codex trust — valid across mAIestro Code updates.
/// On Windows it is a batch file: PowerShell runs a `.cmd` whatever the
/// script execution policy, which a `.ps1` couldn't count on.
pub fn hook_wrapper_path() -> PathBuf {
    crate::paths::maiestro_dir(if cfg!(windows) { "bin/maiestro-hook.cmd" } else { "bin/maiestro-hook" })
}

/// The wrapper script for `bin`: forward every argument to `maiestro hook`.
fn hook_wrapper_script(bin: &Path) -> String {
    if cfg!(windows) {
        windows_hook_wrapper_script(bin)
    } else {
        format!(
            "#!/bin/sh\n# Written by mAIestro Code; runs its status-hook helper.\nexec {} hook \"$@\"\n",
            HookShell::Posix.quote(&bin.to_string_lossy())
        )
    }
}

/// The Windows wrapper: a batch file that forwards its arguments, stdin and
/// exit code to `bin hook`. A path can't contain `"`, and inside quotes only
/// `%` is still special to `cmd`, so that is the one character escaped.
fn windows_hook_wrapper_script(bin: &Path) -> String {
    let bin = bin.to_string_lossy().replace('%', "%%");
    format!("@echo off\r\nrem Written by mAIestro Code; runs its status-hook helper.\r\n\"{bin}\" hook %*\r\n")
}

/// Write (or re-point) the wrapper at [`hook_wrapper_path`] so it runs the
/// current binary. Returns whether it changed. Called on every Codex spawn and
/// reopen (and on Windows every Antigravity one) and at startup.
pub fn ensure_hook_wrapper() -> Result<bool, String> {
    let bin = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    write_hook_wrapper(&hook_wrapper_path(), &bin)
}

fn write_hook_wrapper(path: &Path, bin: &Path) -> Result<bool, String> {
    let script = hook_wrapper_script(bin);
    if std::fs::read_to_string(path).is_ok_and(|s| s == script) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    crate::paths::write_atomic(path, script.as_bytes()).map_err(|e| e.to_string())?;
    // Windows has no execute bit; the `.cmd` extension makes it runnable there.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    Ok(true)
}

/// Append mAIestro Code's generated files to the worktree's shared git exclude file so
/// they don't show up as untracked changes (which would trip teardown's
/// `git status --porcelain` dirty check before the agent has run / in repos that
/// don't already ignore them). Idempotent and best-effort. Antigravity's
/// `.agents/hooks.json` is handled by the worktree's `.gitignore` instead
/// ([`ensure_antigravity_gitignored`]).
async fn exclude_generated_files(work_dir: &Path) {
    // Worktrees share the main repo's exclude via the common git dir; resolve it
    // rather than assuming `<work_dir>/.git` is a directory (in a worktree it's a
    // file pointing elsewhere).
    let Ok(common) = git(work_dir, &["rev-parse", "--git-common-dir"]).await else {
        return;
    };
    let common = expand_tilde(&common);
    let common = if common.is_absolute() { common } else { work_dir.join(common) };
    let exclude = common.join("info").join("exclude");

    let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
    let mut to_add: Vec<&str> = Vec::new();
    for pat in [".claude/settings.local.json", ".vscode/", COPILOT_HOOK_REL] {
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
    body.push_str("# Added by mAIestro Code\n");
    for pat in to_add {
        body.push_str(pat);
        body.push('\n');
    }
    let _ = std::fs::write(&exclude, body);
}

/// Run a hook `command` the way an agent using `shell` would — `sh -c`,
/// `powershell.exe -NoProfile -Command` as Codex does on Windows, or
/// `cmd /c "<command>"` as Antigravity does there — with no stdin. For tests
/// here and in `drafting.rs`.
#[cfg(test)]
pub(crate) fn run_hook_command(shell: HookShell, command: &str) -> std::process::Output {
    let mut cmd = match shell {
        // agy's own command line, verbatim: Go wraps the command in quotes
        // (and would turn an inner `"` into `\"`, which our commands avoid).
        #[cfg(windows)]
        HookShell::Cmd => {
            use std::os::windows::process::CommandExt;
            assert!(!command.contains('"'), "agy would mangle the quotes in {command}");
            let mut c = std::process::Command::new("cmd.exe");
            c.raw_arg("/c").raw_arg(format!("\"{command}\""));
            c
        }
        #[cfg(not(windows))]
        HookShell::Cmd => unreachable!("agy runs hooks with cmd only on Windows"),
        HookShell::Posix => {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", command]);
            c
        }
        HookShell::PowerShell => {
            let mut c = std::process::Command::new("powershell.exe");
            c.args(["-NoProfile", "-Command", command]);
            c
        }
    };
    cmd.stdin(std::process::Stdio::null()).output().unwrap()
}

#[cfg(test)]
/// Run `cmd` as an agent using `shell` would, and require exit 0 and no output.
fn assert_silent_success(shell: HookShell, event: &str, cmd: &str) {
    let out = run_hook_command(shell, cmd);
    assert!(out.status.success(), "{event} failed: {cmd}");
    assert!(out.stdout.is_empty(), "{event} printed {:?}", String::from_utf8_lossy(&out.stdout));
    assert!(out.stderr.is_empty(), "{event} wrote to stderr {:?}", String::from_utf8_lossy(&out.stderr));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both quotings keep a path with spaces and quotes as one word, and
    /// PowerShell calls a program rather than printing its path.
    #[test]
    fn hook_shell_quotes_per_shell() {
        assert_eq!(HookShell::Posix.quote("it's"), r"'it'\''s'");
        assert_eq!(HookShell::PowerShell.quote("it's"), "'it''s'");
        assert_eq!(HookShell::PowerShell.quote("a\u{2019}b"), "'a\u{2019}\u{2019}b'");
        let bin = Path::new(r"C:\Program Files\mAIestro Code\maiestro.exe");
        assert_eq!(
            HookShell::PowerShell.hook_command(bin, "busy", "199-x"),
            r"& 'C:\Program Files\mAIestro Code\maiestro.exe' hook busy --workspace '199-x'"
        );
        assert_eq!(
            HookShell::PowerShell.silent("& 'x' hook busy"),
            "try { & 'x' hook busy *> $null } catch { }; exit 0"
        );
        // On macOS every agent stays on POSIX, so its hook files are unchanged.
        #[cfg(unix)]
        for agent in [Agent::Claude, Agent::Codex, Agent::Antigravity, Agent::Copilot] {
            assert_eq!(HookShell::for_agent(agent), HookShell::Posix);
        }
        // On Windows only Claude (Git Bash) stays POSIX, and agy uses cmd.
        #[cfg(windows)]
        {
            assert_eq!(HookShell::for_agent(Agent::Claude), HookShell::Posix);
            assert_eq!(HookShell::for_agent(Agent::Antigravity), HookShell::Cmd);
            for agent in [Agent::Codex, Agent::Copilot] {
                assert_eq!(HookShell::for_agent(agent), HookShell::PowerShell);
            }
        }
        assert_eq!(HookShell::Cmd.quote(r"C:\a b&(1)%x%,y=z;!^"), r"C:\a^ b^&^(1^)^%x^%^,y^=z^;^!^^");
        assert_eq!(
            HookShell::Cmd.silent(&HookShell::Cmd.hook_command(bin, "busy", "199-x")),
            r"C:\Program^ Files\mAIestro^ Code\maiestro.exe hook busy --workspace 199-x >nul 2>&1 & exit /b 0"
        );
    }

    /// A `cmd` command quoted with [`HookShell::quote`], run as agy runs it,
    /// really calls a program whose path is full of `cmd` metacharacters and
    /// passes an awkward argument through intact.
    #[cfg(windows)]
    #[test]
    fn cmd_quoting_round_trips() {
        let word = "it's a $HOME `x` ; & | < > ( ) 100% !x! ^ , = {}";
        let out = run_hook_command(HookShell::Cmd, &HookShell::Cmd.print_line(word));
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end(), word);

        // A batch file stands in for the binary, echoing its arguments.
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("we ird&dir (1)%PATH%,a=b;c!d^e").join("maiestro.cmd");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, "@echo off\r\necho args: %*\r\n").unwrap();
        let out = run_hook_command(HookShell::Cmd, &HookShell::Cmd.hook_command(&bin, "stop", "199-x"));
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end(), "args: hook stop --workspace 199-x");
    }

    /// A PowerShell command quoted with [`HookShell::quote`] really passes the
    /// awkward argument through intact.
    #[cfg(windows)]
    #[test]
    fn powershell_quoting_round_trips() {
        let word = "it's a \"test\" $HOME `x` ; & | 100%";
        let out = run_hook_command(HookShell::PowerShell, &HookShell::PowerShell.print_line(word));
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end(), word);
    }

    /// The Windows Codex wrapper forwards its arguments, stdin and failure to the
    /// binary; a `%` in the path is escaped for `cmd`.
    #[cfg(windows)]
    #[test]
    fn windows_hook_wrapper_forwards_args_and_stdin() {
        assert!(windows_hook_wrapper_script(Path::new(r"C:\a%b\maiestro.exe")).contains(r#""C:\a%%b\maiestro.exe" hook %*"#));

        // A batch file stands in for the binary, echoing its arguments and stdin.
        let dir = tempfile::tempdir().unwrap();
        let wrapper = dir.path().join("bin with space").join("maiestro-hook.cmd");
        let fake = dir.path().join("fake bin.cmd");
        std::fs::write(&fake, "@echo off\r\necho args=%*\r\nmore\r\nexit /b 3\r\n").unwrap();
        assert!(write_hook_wrapper(&wrapper, &fake).unwrap());
        let command = format!("{} busy", HookShell::PowerShell.program(&wrapper));
        let mut child = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", &command])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        child.stdin.take().unwrap().write_all(b"{\"cwd\":\"x\"}\r\n").unwrap();
        let out = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("args=hook busy"), "{stdout}");
        assert!(stdout.contains(r#"{"cwd":"x"}"#), "stdin reaches the binary: {stdout}");
        // `powershell -Command` reports any native failure as exit 1.
        assert!(!out.status.success(), "a failure still reads as one");
    }

    /// The wrapper execs the given binary's `hook` subcommand, is executable, and
    /// is only rewritten when the binary changes.
    #[test]
    fn hook_wrapper_follows_the_binary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bin/maiestro-hook");
        assert!(write_hook_wrapper(&path, Path::new("/old/maiestro")).unwrap());
        assert!(!write_hook_wrapper(&path, Path::new("/old/maiestro")).unwrap(), "no churn");
        assert!(write_hook_wrapper(&path, Path::new("/Applications/mAIestro Code.app/Contents/MacOS/maiestro")).unwrap());
        let script = std::fs::read_to_string(&path).unwrap();
        #[cfg(windows)]
        assert!(script.contains(r#""/Applications/mAIestro Code.app/Contents/MacOS/maiestro" hook %*"#), "{script}");
        #[cfg(unix)]
        {
            assert!(script.contains(r#"exec '/Applications/mAIestro Code.app/Contents/MacOS/maiestro' hook "$@""#), "{script}");
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o111, 0o111);
        }
    }
}
