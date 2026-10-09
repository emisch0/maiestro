# Live per-session status

Because mAIestro Code launches `claude` but does not host it (see "mAIestro Code launches sessions; it does not host them" in `CLAUDE.md`), it cannot read a session's working/waiting state from stdio. Instead, at worktree creation the hook subsystem (`hooks.rs`, called from the spawn path) writes Claude Code hooks into the worktree's `.claude/settings.local.json` (the personal, gitignored layer that merges with the user's own settings and applies to both terminal and VS Code integrated-terminal sessions). Each hook invokes **the mAIestro Code binary itself** as `maiestro hook <state> --workspace <ws-id>` — a hidden CLI subcommand dispatched in `main()` *before* Tauri starts. Using the app binary as the helper means zero external deps (no `jq`/`python`) and one source of truth; `hooks.rs` bakes its own `current_exe()` path into the generated commands.

That baked path only survives while the spawning build does, so a worktree spawned by a `tauri dev` build (ephemeral `current_exe()`) or a feature worktree that later gets torn down/merged would point its hooks at a missing binary, erroring on every event. To self-heal, **at startup mAIestro Code reconciles every tracked session's hooks** (`hooks::reconcile_all_session_hooks`, called from `setup()` right after `status::sweep_stale`): for each session it rewrites any of *our* hook commands in `.claude/settings.local.json` to the currently-running binary's path, identifying ours by the trailing `--workspace '<ws-id>'` so unrelated hooks are untouched, only when the worktree already carries our hooks, and writing only when the path actually changed. The reuse-existing-workspace spawn path reconciles too, so reopening heals without a restart. Switching a session's agentic coding CLI (`session_agent::session_set_agent`) installs the new agent's hooks with `write_session_hooks` (reconcile never injects hooks into a worktree that has none). When leaving Claude it also removes our Claude hooks for that workspace (`hooks::remove_claude_hooks`). Restarting the window to apply a switch clears the old agent's status record first, so the pill reads Idle until the new agent's `SessionStart`. Release builds resolve to the stable `/Applications/mAIestro Code.app/.../maiestro` and stay fixed; dev builds re-point on each launch. The remaining gap is purely between a spawner disappearing and the next mAIestro Code launch.

The helper reads Claude's hook event JSON on stdin (serde), and atomically writes a status record to `~/.maiestro/status/<ws-id>.json`. The backend watches that directory with the `notify` crate and emits a `session-status` event to the popover; the frontend also reads `sessions_status_list` on open so a reopened popover is correct even if it missed events. The hook → state mapping: `SessionStart`→running, `UserPromptSubmit`/`PreToolUse`/`PostToolUse`/`PostToolUseFailure`→busy, `Notification`→needs_you (detail from the payload message), `Stop`→idle, `SessionEnd`→ended.

**The `creating` state (background spawn).** A fresh spawn is two-phase. `do_spawn` runs a fast synchronous phase — resolve the final workspace id, write the `Session` record, and write a `state: "creating"` status (`status::write_creating`) — then hands the slow work (worktree add, env copy, GitHub assign/comment, `post_spawn_commands`, editor launch) to a background `tokio::spawn` (`finish_spawn`) and returns at once. So `confirm_spawn` resolves quickly, the popover lands on the dashboard immediately, and the new row shows a "Creating…" `workspace-op-pill` (the same monochrome busy-ring pill as "Merging…"/"Tearing down…", not an AI/Claude status pill) until the worktree is ready. `creating` is mAIestro Code's own pre-Claude state, not a hook state: `finish_spawn` clears it (`status::clear_creating`, which only removes a still-`creating` record) right before opening the editor, handing the status over to Claude's own hooks; on a fatal failure it instead writes a surfaced `last_error` (`status::write_spawn_error`) so the broken row shows the usual dismissible error block and can be torn down (teardown tolerates a never-created worktree). Reopening an existing worktree stays fully synchronous — no `creating` pill. `PostToolUse`→busy is what flips a session out of "needs you" after an approved permission prompt: the prompt's `Notification` sets needs_you, and `PostToolUse` (fired once the approved tool completes) is the only signal that Claude has resumed working — `PreToolUse` can't serve this, since it fires *before* the prompt, not after approval. The one unavoidable gap is the in-flight execution of a permission-gated tool: there is no hook at the moment of approval, so the pill reads needs_you until that tool finishes. The helper is deliberately failure-tolerant — a hook must never block or crash the user's session.

**Failed tool calls.** `PostToolUseFailure` fires after a tool call fails. The state stays `busy` (Claude works on past the failure), but the helper also captures the error into a separate `last_error` field on the status record (`{ tool, message, ts, count, surfaced }`) and logs it to the standard backend log (`logging::append_line` → today's `~/Library/Logs/com.maiestro.app/…` file; the hook subprocess writes there directly since it's too short-lived to install the `tracing` subscriber). Because the status file is last-write-wins and the next hook lands within seconds, `last_error` can't live in `state`: instead the helper does a **read-merge-write**.

Crucially, **not every failure is shown.** Claude often recovers from a transient failure (a flaky network tool, a timeout it retries) and keeps working, so surfacing each one immediately is noise. Every failure is *logged*, but the popover only displays `last_error` once `surfaced` is true. The lifecycle:
- `PostToolUseFailure`→`tool_failed` *sets* it as a **pending** (`surfaced: false`) error. `count` tracks consecutive failures of the *same* tool; a repeated failure (`count >= RETRY_SURFACE_THRESHOLD`, currently 2) is persistent and surfaces immediately.
- `PostToolUse`→`tool_ok` (a tool *succeeded*) *clears* a pending error — Claude recovered. An already-surfaced error stays until dismissed. This is why `PostToolUse` maps through its own `tool_ok` verb rather than sharing `busy` with `PreToolUse`: only `tool_ok` means a tool finished successfully, the signal that distinguishes recovery from a retry attempt.
- `Stop`→`idle` *promotes* a pending error to `surfaced: true` — Claude stopped without recovering, so the failed call was effectively its last action and the user should see it.
- A new turn or session (`UserPromptSubmit`→`prompt`, `SessionStart`→`running`) *clears* it. (This is why `UserPromptSubmit` maps through its own `prompt` verb rather than sharing `busy`.)
- Every other event *carries it forward*.

The popover shows a *surfaced* `last_error` as a dismissible inline block (the same `.cleanup-confirm` pattern PR-create errors use, never a tooltip) and tints the agent pill red; a pending one stays hidden. The **Dismiss** button calls the `clear_session_error` command, which rewrites the record without `last_error` so it doesn't reappear on reopen. The exact error field in the `PostToolUseFailure` payload isn't pinned down in the docs, so `extract_error_message` reads several likely fields in order and falls back to a generic message. Because `reconcile_all_session_hooks` rebuilds hook commands from `maiestro_hook_groups` on every startup, a change to the hook set or its verbs reaches already-spawned worktrees on the next app launch, no re-spawn needed.

The whole mechanism is event-driven and last-write-wins, assuming one session per worktree (keyed by `<ws-id>`, which equals `Session.id`). The same `.claude/settings.local.json` also carries a `statusLine` entry (`maiestro statusline --workspace <ws-id>`) that records the Claude subscription quota and then chains the user's own status line; it is installed, re-pointed and removed alongside the hooks, and is never written over a `statusLine` the file already has. See `docs/provider-quotas.md`. Generated files (`.claude/settings.local.json`, `.vscode/`) are added to the worktree's shared git exclude (`$(git rev-parse --git-common-dir)/info/exclude`) so they don't trip teardown's `git status --porcelain` dirty check before Claude has run.

## Windows

Every agent gets the same hook events and verbs on Windows. What changes is the shell the agent runs a hook `command` with, so each command is quoted for **that agent's** shell, not for the OS mAIestro Code runs on (issue #199). `hooks::HookShell` (`Posix`, `PowerShell` or `Cmd`, from `HookShell::for_agent`) builds the hook commands and matches our own entries (`is_maiestro_hook`). Hooks don't use the OS-wide `tools::shell_quote`, so a change there can't break a hook that runs in a different shell. On macOS every agent stays `Posix`, so the generated files and the Codex overrides are byte-identical to before, and existing Codex trust still holds.

| Agent | Shell on Windows | How we know | What we write |
|---|---|---|---|
| Claude Code | Git Bash (`bash -c`) | Tested on Windows 11 with Claude Code 2.1.295. A `SessionStart` hook reported `$0=/usr/bin/bash`, and a single-quoted `'C:\…\dir with space\bin.exe'` ran and got the JSON payload on stdin. Claude's docs say status lines use Git Bash too. | The POSIX commands and status line, unchanged. |
| Codex | `pwsh -NoProfile -Command`, else `powershell.exe` | Codex source (`codex-rs/hooks/src/engine/command_runner.rs`; on Windows the user shell is always PowerShell). Not yet run against a real Codex here. | `& '<…>\maiestro-hook.cmd' <verb>` |
| Copilot | Its handler's `powershell` field (PowerShell) | Copilot's hook docs: `bash` on Unix, `powershell` on Windows. | Both fields. `powershell` is added only on Windows. |
| Antigravity | `cmd /c "<command>"` | Tested on Windows 11 with agy: a probe hook recorded `%CMDCMDLINE%`, and agy's log (`~/.gemini/antigravity-cli/cli.log`) showed the earlier PowerShell forms failing with `'try' is not recognized`. | `<…>\maiestro-hook.cmd <verb> --workspace <ws-id>`, `^`-escaped (the `cmd` form). |

**The PowerShell forms** (Codex, Copilot).
- **Running the program.** A quoted program path needs PowerShell's call operator: `& 'C:\…\maiestro.exe' hook <verb> --workspace '<ws-id>'`. A bare quoted string would just be printed. Single quotes are escaped by doubling them, including the typographic ones PowerShell also treats as quotes.
- **The silent form** for Copilot is `try { <command> *> $null } catch { }; exit 0`. A plain `*> $null` isn't enough: PowerShell reports a missing program before the redirection applies, which would print an error. The `try` keeps the hook silent and exiting 0 even when the baked binary is gone. A unit test runs each command through `powershell.exe` against a missing binary to prove it.

**The `cmd` form** (Antigravity). agy builds its `cmd /c "<command>"` line Go-style, so every `"` inside the command reaches `cmd` as a literal `\"`, which it can't parse. Our commands therefore contain no double quotes.
- **Escaping.** A path is made literal by putting `^` before every ASCII character except letters, digits and `\ / : . _ -`. That covers spaces and `& ( ) % , ; = ! ^`.
- **Calling the wrapper, not the binary.** `cmd` strips the carets and then starts a program with its path *unquoted*, so the program's own argument parsing splits a path with a space: the helper would get `dir` instead of `hook` and start as the app. A batch file's arguments are parsed by `cmd` itself, so agy's hooks call the same stable wrapper as Codex (`~/.maiestro/bin/maiestro-hook.cmd`, `"<bin>" hook %*`), which quotes the binary inside the file. Writing or reconciling an Antigravity worktree's hooks re-points the wrapper at the running binary first (`hooks::antigravity_hook_program`).
- **The silent form** is `<command> >nul 2>&1 & exit /b 0`. `cmd` reports a missing program on the redirected stderr, and `&` runs `exit /b 0` whatever happened. A unit test runs each command as agy does (`cmd /c "…"`) against a missing program, and another runs a stand-in batch file in a folder named with every metacharacter above.
- **The drafting lockdown hook** (`drafting::antigravity_lockdown_hooks`) prints `{}` with `echo`, since the JSON decision needs quotes. agy reads `{}` as a deny too.

**The Codex wrapper on Windows** is `~/.maiestro/bin/maiestro-hook.cmd`, a batch file whose one real line is `"<bin>" hook %*`:
- A `.cmd` runs whatever PowerShell's script execution policy is, which a `.ps1` couldn't count on.
- It forwards its arguments, stdin and failure to the binary. A unit test checks this through `powershell -Command`.
- A `%` in the binary path is doubled. That's the one character still special to `cmd` inside quotes, and a path can't contain `"`.
- Like the macOS script, it is rewritten only when the binary changes. Its path never changes, so the Codex hook definitions, and the user's one-time trust, stay valid across updates.

**The helper itself.** The release `maiestro.exe` is a GUI-subsystem binary, so a hook starting it opens no console window. Its stdin and stdout are the pipes the agent passes, and the helper still reads the payload and writes the record.

**Finding the session from `cwd`.** `status::workspace_for_cwd` (used by Codex's workspace-less hooks) compares paths with `/` separators and, on Windows, case-insensitively. A Codex `cwd` like `c:\src\work-8-a\repo\frontend` therefore matches the worktree recorded as `C:\src\work-8-a\repo`.

**Known gaps on Windows:**
- **No Git Bash, no Claude status.** Claude Code falls back to PowerShell for hooks when Git Bash isn't installed, and our POSIX commands would then just print a string. mAIestro Code already relies on Git Bash on Windows (`tools::git_bash`; see `docs/provider-quotas.md`).
- **Only Claude and Antigravity have been tested end to end on Windows.** For Antigravity that means a headless agy run whose `PreInvocation` hook, in the `cmd` form above, reached the real helper through the wrapper and wrote the record. The Codex and Copilot forms are tested against `powershell.exe` in unit tests, not yet against the agents themselves.

## Codex sessions

A repo whose agentic coding CLI is Codex gets the same status helper and verbs, delivered differently, because of how Codex trusts hooks.

**Why not a per-worktree hook file.** Codex runs a non-managed hook only after the user reviews it, and records that approval in `~/.codex/config.toml` as `hooks.state."<key>".trusted_hash` — the key names the hook's source and position, the hash covers its exact definition. A per-worktree `.codex/hooks.json` with a `--workspace <id>` in each command would be a *new* hook to Codex on every spawn, so every workspace would need its own review. (It also doesn't load at all: `hooks/list` on the app-server shows zero hooks for a spawned worktree's `.codex/hooks.json`, even with the folder trusted.)

**What we do instead.** The hooks ride on the launch command as `-c hooks.<Event>=[…]` session-flag overrides (`hooks::codex_hook_overrides`, added to the VS Code task by `editor::session_command`). Session-flag hooks have trust keys with no folder path in them (`/<session-flags>/config.toml:stop:0:0`), and every command is identical for every worktree: it runs a **stable wrapper**, `~/.maiestro/bin/maiestro-hook <verb>`, with no workspace id. So the definitions, and their hashes, never change. We checked this with the app-server's `hooks/list`: the key and hash are the same in every folder, and one stored approval marks them trusted everywhere. The helper finds the session from the payload's `cwd` instead (`status::workspace_for_cwd`: the tracked worktree that contains it, deepest match first). A `cwd` in no tracked worktree records nothing. The wrapper is a two-line `sh` script (a `.cmd` batch file on Windows, see "Windows" above) that execs the running mAIestro Code binary's `hook` subcommand. Spawn, reopen and startup reconcile rewrite its *contents* (`hooks::ensure_hook_wrapper`), never the hook definitions, so an app update or a `tauri dev` rebuild never invalidates the user's approval. Nothing is written into the worktree or into `~/.codex/`.

**One-time trust.** The first time a Codex session starts with these hooks, Codex immediately asks the user to review them; choosing **trust all** once is enough (`/hooks` reopens the same review later). Codex saves the approval itself, and it covers every later workspace. So the user knows what to expect, the popover explains this step in a short dialog (`CodexHooksDialog`) *before* VS Code opens a Codex session, from a spawn or from Open in editor, but only when Codex really will prompt. `hooks::codex_hooks_review_needed` asks Codex itself, read-only: one `initialize` + `hooks/list` round trip with `codex app-server`, passing the same `-c` hook overrides a session gets (~0.1 s). If any of our session-flag hooks is `untrusted` or `modified`, the dialog shows. Otherwise, or if the check fails, the action just proceeds. mAIestro Code never writes the approval for you and never passes `--dangerously-bypass-hook-trust`, which would also run any hooks a cloned repo ships in its own `.codex/`.

| Codex event | verb |
|---|---|
| `SessionStart` | `running` |
| `UserPromptSubmit` | `prompt` |
| `PreToolUse` | `busy` |
| `PostToolUse` | `tool_ok` |
| `PermissionRequest` | `notification` |
| `Stop` | `idle` |
| `SessionEnd` | `ended` |

- **No `last_error` for Codex.** Codex has no `PostToolUseFailure` event, so a Codex session never gets a failed-tool error or a red pill. This is documented, not emulated (no parsing of `PostToolUse` responses).
- **`PermissionRequest` stands in for `Notification`.** Codex has no `Notification` event. Its `PermissionRequest` payload carries `tool_name`/`tool_input` but no `message`, so `resolve_state` falls back to a detail of ``Permission requested: `<tool_name>` `` for the pill's tooltip.
- **Drafting doesn't report status.** mAIestro Code's own `codex exec` drafting calls run with `--disable hooks`, so a PR draft run inside a Codex worktree doesn't flip the session's pill.

## Antigravity sessions

A repo whose agentic coding CLI is Antigravity (`agy`) uses the same status helper and records, fed by Antigravity's own hook system. Its events are coarser than Claude's, and a few of its behaviours shape what we install.

**Where the hooks live.** Antigravity reads workspace hooks from `<worktree>/.agents/hooks.json`, a JSON object of *named* hook groups. At spawn, `hooks::write_session_hooks` sets one group, `maiestro-status` (`hooks::antigravity_hook_group`), and keeps every other group. So a repo that commits its own `.agents/hooks.json` keeps its hooks. A file that exists but isn't a JSON object is left alone, and that session simply shows no status. Like Claude's, each command bakes in the running binary and `--workspace <id>`, and startup and every reopen rewrite the group to the current binary (`reconcile_antigravity_hooks_with`), but only when the group is already there. We never write `~/.gemini/` (neither the global `config/hooks.json` nor Antigravity's settings).

**Kept out of git by `.gitignore`.** The file holds this machine's binary path and workspace id, so it must never be committed. Unlike Claude's `.claude/settings.local.json` (in the local `info/exclude`), `.agents/hooks.json` is ignored through the worktree's tracked **`.gitignore`**, so the rule also reaches collaborators and fresh clones. `hooks::ensure_antigravity_gitignored` runs `git check-ignore -v --no-index`, and only a match from a `.gitignore` counts: `info/exclude`, the global excludes file, or a `!` negation don't. If nothing covers the file, it appends a commented `.agents/hooks.json` line to the worktree-root `.gitignore`, creating the file if needed. Then it records a **notice** on the session record (`Session.notice`), which the popover shows on the work item as a dismissible block (`session_dismiss_notice`) telling the user to commit the change. It runs on spawn and on switching to Antigravity, and in every reconcile (startup and reopen) for a worktree that has our hooks, so a deleted line is put back and the notice shown again. A `.gitignore` can't hide a file the repo already **tracks**. In that case nothing is appended, and on spawn and switch (not every reconcile) the notice instead warns that our group shows up as a change to keep out of commits. The spawn runs in the background, so the popover re-reads the session list when a worktree leaves `creating`, which is when the notice becomes visible.

**Folder trust.** An interactive `agy` loads workspace hooks only in a trusted folder, and trust is recorded per exact path (`~/.gemini/antigravity-cli/settings.json` → `trustedWorkspaces`). A trusted parent such as `~/src` doesn't cover a worktree inside it. So every new worktree gets Antigravity's own "Do you trust the contents of this project?" prompt when the session starts. Antigravity asks that for any new folder whether or not we add hooks, so mAIestro Code shows no dialog of its own. Once the user answers yes, the hooks load.

**A `PreToolUse` hook's output is a decision.** Unlike Claude, Antigravity reads a `PreToolUse` hook's stdout as a permission decision:
- `{}` or `{"decision":""}` **denies** the tool
- a non-zero exit **blocks** it with a hook-failed error
- only **empty stdout with exit 0** leaves the tool to its normal permission flow (`"ask"` would force a prompt, and `"allow"` would auto-approve)

So every command we install ends in `>/dev/null 2>&1 || true` (`>nul 2>&1 & exit /b 0` on Windows; see "The `cmd` form" above). It is silent and succeeds even if the baked binary has been deleted, and a unit test runs each command against a missing binary to prove it. `PreToolUse` is also registered **only** for the tools that ask the user something and the tools that can raise agy's own permission prompt, which keeps any hook problem away from ordinary tools. `Stop` reads `"decision":"continue"` as "keep going" and anything else as "stop", so silence is neutral there too.

| Antigravity event | verb | state |
|---|---|---|
| `PreInvocation` (before every model call) | `invocation` | `prompt` for the first call of a turn (`invocationNum` 0: a new turn clears a stale error), else `busy` |
| `PreToolUse`, matcher `ask_question\|ask_permission\|ask_custom_permission` | `notification` | `needs_you`; the detail is the question text, or ``Permission requested: `<tool>` `` |
| `PreToolUse`, matcher `run_command\|write_to_file\|replace_file_content\|multi_replace_file_content` | `gated` | `busy`, plus a `prompt_after` deadline; `needs_you` once it passes with no other hook (see below) |
| `PostToolUse`, matcher `*` | `tool_done` | `tool_failed` when the payload's `error` is non-empty, else `tool_ok` |
| `Stop` | `stop` | `idle`; a non-empty `error` becomes a surfaced `last_error` |

**agy's own permission prompts.** "Run this command?", "Allow creation of this file?" and "Accept this file edit?" fire no hook, and the tool's `PreToolUse` fires *before* the prompt, with nothing in the payload to say one is coming. So for the tools that can prompt, the `gated` hook records `busy` plus a `prompt_after` deadline on the status record (`status::pending_prompt`). For `run_command` the deadline is the payload's `WaitMsBeforeAsync` (default 5 s, capped at 60 s) plus 2 s. For a file write or edit it is 3 s. When agy *doesn't* prompt, another hook always lands before then. A short command ends with `PostToolUse`. A long one is moved to the background after `WaitMsBeforeAsync`, and agy calls the model again (`PreInvocation`); its `PostToolUse` only comes when it finally exits. A file write or edit finishes at once. The helper never carries `prompt_after` forward, so any later hook clears it. When the record is still unchanged at the deadline, the backend promotes it to `needs_you`, with the pending command (secrets masked, shortened) or ``Permission requested: `<tool>` `` as the detail (`status::promote_overdue`). The watcher sets a timer for that, and `sessions_status_list` also promotes, and saves, an overdue record the timer missed (e.g. the app wasn't running). Approving the prompt lets the tool finish, and its next hook returns the pill to *Working*. Declining it fires nothing, so the pill stays on *Needs you*, which is right: agy then asks what to do instead.

The payload-dependent verbs are resolved in `status::normalize_verb`. The payload is camelCase: the tool is `toolCall.name` (not `tool_name`), the session id is `conversationId`, and `workspacePaths[0]` stands in for `cwd`.

Known gaps, documented and not emulated:
- **No session start or end.** A session shows no pill until its first prompt, and "ended" is never reported (teardown still clears the row).
- **"Needs you" for agy's own permission prompts is inferred, and late.** It appears only after the deadline above (as much as `WaitMsBeforeAsync` + 2 s after the prompt shows), and only for the gated tools. After approval, the pill keeps reading *Needs you* while the approved tool runs, until its next hook (the same gap Claude has).
- **Esc fires nothing.** Interrupting a turn fires no `Stop`, so the pill keeps its last state (usually *Working*) until the next turn.
- **`last_error` is rare.** `PostToolUse` reports `"error": ""` even for a shell command that exited non-zero (the tool itself ran), and a tool that errors outright (e.g. `view_file` on a missing file) fires no `PostToolUse` at all.
- **No session name or color.** `agy` has neither a session-name flag nor `/color` (see `docs/theming.md`).
- **Drafting doesn't report status.** mAIestro Code's headless `agy` drafting runs in its own folder, `~/.maiestro/antigravity-draft/`, never a worktree, so it loads none of a worktree's hooks.

## Copilot sessions

A repo whose agentic coding CLI is GitHub Copilot CLI (`copilot`) uses the same status helper and records, fed by Copilot's repo hooks.

**Where the hooks live.** Copilot reads repo hooks from `<worktree>/.github/hooks/*.json`, any filename, as `{"version": 1, "hooks": {"<event>": [{"type": "command", "bash": "…", "timeoutSec": N}]}}`. So mAIestro Code owns a whole file, `.github/hooks/maiestro-status.json` (`hooks::copilot_hooks`), and never merges into anyone else's. `write_session_hooks` writes it at spawn. Startup and every reopen re-point it at the running binary (`reconcile_session_hooks`), but only when the file is already there. Switching a session away from Copilot deletes it (`hooks::remove_copilot_hooks`), along with `.github/hooks/` and `.github/` if that left them empty. Each command bakes in the binary and `--workspace <id>`, like Claude's.

**Kept out of git via `info/exclude`.** The filename is mAIestro-only, so it goes in the repo's local `info/exclude` next to `.claude/settings.local.json`. Unlike Antigravity's shared `.agents/hooks.json`, it never needs a tracked `.gitignore` line or a notice.

**Folder trust.** Copilot loads repo hooks only in a trusted folder: in an untrusted one the file is silently ignored. An interactive `copilot` asks *"Do you trust the files in this folder?"* in every new worktree whether or not we add hooks, so mAIestro Code shows no dialog of its own, and the pill appears once the user says yes. Trust is recorded in `~/.copilot/config.json` → `trustedFolders`, which we never write, and we never set `COPILOT_ALLOW_ALL` (it would also auto-approve every tool).

**Silent hooks are neutral.** Copilot reads a `preToolUse` or `permissionRequest` hook's output as a decision. Every command we install ends in `>/dev/null 2>&1 || true` (`hooks::silent_hook_command`, shared with Antigravity), so it neither approves nor denies a tool, even if the baked binary has been deleted. A unit test runs each command against a missing binary to prove it.

| Copilot event | verb | state |
|---|---|---|
| `sessionStart` | `session_start` | `running`, unless the same Copilot session is already `busy`/`needs_you` (it can fire *after* `userPromptSubmitted`), which is then left alone |
| `userPromptSubmitted` | `prompt` | `busy`, clearing a stale error |
| `preToolUse` | `busy` | `busy`, naming the tool |
| `permissionRequest` | `notification` | `needs_you`; the detail is ``Permission requested: `<toolName>` `` |
| `postToolUse` | `tool_ok` | `busy` |
| `postToolUseFailure` | `tool_failed` | `busy`; the payload's `error` string feeds the persistent-failure logic |
| `errorOccurred` | `error` | `busy`, with a surfaced `last_error` (documented by Copilot, not yet seen in practice) |
| `agentStop` | `idle` | `idle` |
| `sessionEnd` | `ended` | `ended` |

The payload is camelCase: the tool is `toolName`, the session id is `sessionId` (`status::payload_session_id`), and `cwd` is present. Copilot ignores event names it doesn't know, so an older CLI simply skips the ones it lacks.

Known gaps, documented and not emulated:
- **No pill before the folder is trusted.** Until the user answers Copilot's trust prompt, no hook runs.
- **A denied tool fires no post-tool event**, so the pill reads *Working* until the turn's next event.
- **`last_error` covers tool-level failures only.** A shell command that exits non-zero is still a successful `postToolUse` (`resultType: "success"`), as with Antigravity.
- **No session color.** Copilot has no `/color` (see `docs/theming.md`).
- **Drafting doesn't report status.** mAIestro Code's headless `copilot` drafting runs in its own untrusted folder, `~/.maiestro/copilot-draft/`, never a worktree, so it loads none of a worktree's hooks.
