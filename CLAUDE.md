# mAIestro

## Shell commands

Never hardcode `/Users/<name>/` in Bash commands. Use relative paths or `~` instead (e.g. `~/src/maiestro`, `./backend`). A hook blocks any command containing a hardcoded `/Users/` path.

A Tauri v2 menu-bar app on macOS that launches git-worktree-per-issue development workspaces, opening a Claude Code session in VSCode or a terminal for each. mAIestro is a launcher and dashboard, not a session host.

Directory layout: `backend/` (Rust/Tauri) and `frontend/` (web). Not Tauri's defaults of `src-tauri/` and `src/`.

The frontend uses **pnpm** (`pnpm-lock.yaml`), not npm or yarn. Use `pnpm install` / `pnpm dev` — `npm install` fails on the `link:` workspace deps.

## Architectural decisions

### mAIestro launches sessions; it does not host them

mAIestro is a **launcher and dashboard**, not a session host. The popover lists tracked repos and their open issues and lets the user start work — it does **not** contain the chat with Claude.

We tried the opposite first: hosting the conversation in the popover by running `claude` headlessly with its stdio piped to the backend, and letting editors attach to that running session. That did not work out, so it is abandoned. There is no headless `claude` subprocess, no stdio piping, and no `SessionRegistry` owning live conversations.

Instead, starting work creates the worktree and **launches `claude` with the right command-line arguments into a real, user-facing session** — either a VSCode window or a standalone terminal (selected per-repo). The conversation lives in that terminal/editor and the user interacts with it directly.

Consequence: because mAIestro no longer owns the process or its stdio, it cannot directly observe a session's live working/waiting state. Surfacing that later would need a separate, out-of-band mechanism (e.g. Claude Code hooks writing status to a file mAIestro watches), not stdio.

### Talk to GitHub directly, not via `gh`

mAIestro uses the GitHub REST API directly (via `octocrab` or a thin `reqwest` wrapper) with per-profile tokens read from Keychain at call time. We do **not** shell out to `gh`.

Reasons:
- `gh auth login` has a single active account, which conflicts with the multi-agent / per-profile identity model.
- At app launch the shell environment is not sourced and `$PATH` is minimal, so `gh` may not even be discoverable.
- Tokens already need to live in Keychain for the profile model; routing them through `gh` adds a layer with no benefit.

This applies only to mAIestro's **own** API calls. The user's `git` operations (fetch/push) happen inside the launched session under the user's ambient git auth, not through mAIestro. mAIestro's own local git (e.g. `git worktree add`) runs in the existing checkout and relies on its already-configured auth.

### Identity = AgentProfile, stored in Keychain

The unit of identity is an `AgentProfile`: name, allowed repos, capability set, and references to credentials. The credential **values** (currently the GitHub token) live in the macOS Keychain under `com.maiestro.agent.<profile-id>.<credential-kind>`. The profile JSON in `~/.maiestro/profiles.json` holds only the references. This path is intentionally developer-friendly (like `~/.ssh/`) so profiles can be inspected, edited by hand, and managed by dotfile tooling.

Keychain is chosen because it is unlocked at user login, so credentials are available even when mAIestro launches at startup (when shell profiles are not sourced and env files are unavailable).

### Launched sessions use the user's ambient environment

mAIestro does **not** build a clean per-profile env or inject credentials into the session it launches. Launching goes through macOS Launch Services (`open -a <App> <worktree>`), exactly like a Finder double-click, so the editor/terminal — and the `claude` running inside it — inherits the **user's full ambient environment**: Homebrew PATH, shell integrations, and whatever git/GitHub auth the user already has. Claude authentication likewise comes from the user's own `claude` login (`~/.claude/`). mAIestro stores and manages no `ANTHROPIC_API_KEY`.

We deliberately dropped the per-session GitHub identity isolation that an injected env would give. The simpler launch path wins; the trade-off is that launched dev sessions act as the user's ambient GitHub identity, so there is effectively one active GitHub identity per machine for the sessions themselves.

Keychain-stored tokens still matter, but **only for mAIestro's own GitHub API calls** (listing a repo's issues under a chosen identity) — never injected into the launched session. See "Identity" and "Talk to GitHub directly".

### Code signing is required for releases (and for Keychain trust)

Release builds must be signed with a **Developer ID Application** certificate and notarized. Beyond distribution, this is also what makes the app's Keychain access usable: macOS binds a Keychain item's "Always Allow" decision to the app's *designated requirement*. For an ad-hoc/unsigned build that requirement is the binary's cdhash, which changes on every rebuild — so the access prompt returns each launch. A stable Developer ID signature anchors the requirement to the certificate, so the grant persists.

Dev builds (`tauri dev`) run the raw, ad-hoc-signed binary and will re-prompt on each rebuild — that is expected and accepted. Signing is **not** committed to `tauri.conf.json`; the signing identity and notarization secrets live in a gitignored `.env.release` (template: `.env.release.example`). Run `scripts/release.sh`, which sources that file, builds via `tauri build`, and verifies the signature/notarization.

### Backend logging

The Rust backend logs via `tracing` (`backend/src/logging.rs`). `logging::init()` runs first in `main()` and installs two layers: a **daily, UTC-dated file** and **stderr** (so `tauri dev` / `cargo run` show output in the terminal).

Log files live at:

```
~/Library/Logs/com.maiestro.app/lYYYYMM/maiestro-YYYYMMDD.log
```

The monthly directory (`lYYYYMM`) and the filename (`maiestro-YYYYMMDD.log`) are both derived from the **current UTC date**, and every log line's timestamp is UTC (RFC 3339). The file rolls over at UTC midnight even while the long-running menu-bar process keeps going — which is why we use a custom `MakeWriter` rather than `tauri-plugin-log` (its path is fixed at startup and it only rotates by size). To read today's log on a packaged build, open or `tail` that file — no rebuild needed.

Conventions:
- **Every Tauri command** logs its invocation via a `log_invoke!`/`log_invoke_debug!` macro (first statement of the command), naming the command and key args. Action commands (spawn, teardown, create PR/issue, set/delete) log at `info`; read-only "get status" commands the UI polls (`sessions_list`, `session_pr`, `repos_list`, `repo_settings_get`, `identities_*`, `credentials_get`, `github_list_*`, …) log at `debug` so they don't drown the `info` log — set `RUST_LOG=debug` to see them.
- **Every GitHub API call** logs method + URL + status from the single `GitHub::send` choke point in `plugins/github.rs`: GETs (read-only, polled) at `debug`, mutations (POST/PATCH/DELETE) at `info`. A non-2xx response logs at `error` regardless.
- **Workspace-scoped commands** (`teardown`, `session_pr`, `session_create_pr`, and the spawn path via `do_spawn`) are wrapped in a `#[tracing::instrument]` span carrying a `session=<workspace-id>` field. Because the span follows the async work across `.await`s, every nested line it emits — git ops, GitHub API calls, Claude drafts — carries the same `session=`, so you can `grep 'session=28-add-foo'` to see one workspace's whole story. Commands with no workspace (e.g. `repos_list`, `github_list_repos`) have no `session` field.
- **Never log credentials.** Command invocations log credential *types* and identity scopes but never secret values; the GitHub token lives only in the `Authorization` header, which is never logged.
- Default level is `info`; override with the `RUST_LOG` env var (standard `EnvFilter` syntax, e.g. `RUST_LOG=maiestro=debug`).
- Old daily files are not auto-pruned yet (cleanup is a possible follow-up).

### All launches use `open -a`, not a constructed env

Every launch hands off to the OS rather than building an env: opening an app uses Launch Services (`open -a <App> <worktree-path>`), and starting `claude` in a standalone terminal uses the terminal's own run-command (e.g. `osascript … do script "cd <worktree> && claude …"`), which runs under the user's login shell. Either way the session inherits the user's full environment (Homebrew PATH, shell integrations, all installed tools) with no env construction by mAIestro. There is no separate "agent spawn with a constructed env" path; everything mAIestro launches gets the *user's* world.

Which app a session opens in (a specific terminal, an editor) and any workspace-level env files are stored in per-repo settings — see "Per-repo settings" below.

One deliberate exception: when VS Code is available, we open worktrees via its `code` CLI (located on `$PATH`, falling back to common install paths and the bundled `.app/Contents/Resources/app/bin/code`) instead of `open -a`, so we can pass `--disable-workspace-trust` and skip the "Do you trust the authors?" prompt on every freshly spawned worktree. The `code` CLI forwards that flag even to an already-running VS Code, which `open -a --args` cannot. If no `code` CLI is found we fall back to `open -a "Visual Studio Code" --args --disable-workspace-trust <worktree>`. The session still inherits the user's ambient environment either way.

### Live per-session status via Claude Code hooks

Because mAIestro launches `claude` but does not host it (see above), it cannot read a session's working/waiting state from stdio. Instead, at worktree creation `spawn.rs` writes Claude Code hooks into the worktree's `.claude/settings.local.json` (the personal, gitignored layer that merges with the user's own settings and applies to both terminal and VS Code integrated-terminal sessions). Each hook invokes **the mAIestro binary itself** as `maiestro hook <state> --workspace <ws-id>` — a hidden CLI subcommand dispatched in `main()` *before* Tauri starts. Using the app binary as the helper means zero external deps (no `jq`/`python`) and one source of truth; `spawn.rs` bakes its own `current_exe()` path into the generated commands.

That baked path only survives while the spawning build does, so a worktree spawned by a `tauri dev` build (ephemeral `current_exe()`) or a feature worktree that later gets torn down/merged would point its hooks at a missing binary, erroring on every event. To self-heal, **at startup mAIestro reconciles every tracked session's hooks** (`spawn::reconcile_all_session_hooks`, called from `setup()` right after `status::sweep_stale`): for each session it rewrites any of *our* hook commands in `.claude/settings.local.json` to the currently-running binary's path, identifying ours by the trailing `--workspace '<ws-id>'` so unrelated hooks are untouched, only when the worktree already carries our hooks, and writing only when the path actually changed. The reuse-existing-workspace spawn path reconciles too, so reopening heals without a restart. Release builds resolve to the stable `/Applications/mAIestro.app/.../maiestro` and stay fixed; dev builds re-point on each launch. The remaining gap is purely between a spawner disappearing and the next mAIestro launch.

The helper reads Claude's hook event JSON on stdin (serde), and atomically writes a status record to `~/.maiestro/status/<ws-id>.json`. The backend watches that directory with the `notify` crate and emits a `session-status` event to the popover; the frontend also reads `sessions_status_list` on open so a reopened popover is correct even if it missed events. The hook → state mapping: `SessionStart`→running, `UserPromptSubmit`/`PreToolUse`/`PostToolUse`/`PostToolUseFailure`→busy, `Notification`→needs_you (detail from the payload message), `Stop`→idle, `SessionEnd`→ended. `PostToolUse`→busy is what flips a session out of "needs you" after an approved permission prompt: the prompt's `Notification` sets needs_you, and `PostToolUse` (fired once the approved tool completes) is the only signal that Claude has resumed working — `PreToolUse` can't serve this, since it fires *before* the prompt, not after approval. The one unavoidable gap is the in-flight execution of a permission-gated tool: there is no hook at the moment of approval, so the pill reads needs_you until that tool finishes. The helper is deliberately failure-tolerant — a hook must never block or crash the user's session.

**Failed tool calls.** `PostToolUseFailure` fires after a tool call fails. The state stays `busy` (Claude works on past the failure), but the helper also captures the error into a separate `last_error` field on the status record (`{ tool, message, ts }`) and logs it to the standard backend log (`logging::append_line` → today's `~/Library/Logs/com.maiestro.app/…` file; the hook subprocess writes there directly since it's too short-lived to install the `tracing` subscriber). Because the status file is last-write-wins and the next `busy`/`idle` hook lands within seconds, `last_error` can't live in `state`: instead the helper does a **read-merge-write** — a failed tool *sets* it, a new turn or session (`UserPromptSubmit`→`prompt`, `SessionStart`→`running`) *clears* it, and every other event *carries it forward*. (This is why `UserPromptSubmit` maps through its own `prompt` verb rather than sharing `busy` — so it can clear the error while still reading as working.) The popover shows `last_error` as a dismissible inline block (the same `.cleanup-confirm` pattern PR-create errors use, never a tooltip); the **Dismiss** button calls the `clear_session_error` command, which rewrites the record without `last_error` so it doesn't reappear on reopen. The exact error field in the `PostToolUseFailure` payload isn't pinned down in the docs, so `extract_error_message` reads several likely fields in order and falls back to a generic message.

The whole mechanism is event-driven and last-write-wins, assuming one session per worktree (keyed by `<ws-id>`, which equals `Session.id`). Generated files (`.claude/settings.local.json`, `.vscode/`) are added to the worktree's shared git exclude (`$(git rev-parse --git-common-dir)/info/exclude`) so they don't trip teardown's `git status --porcelain` dirty check before Claude has run.

### Per-repo settings

Each repo tracked by mAIestro has a small settings record stored in `~/.maiestro/repos/<owner>-<name>.json`. This is the place for configuration that is specific to a repo but not a credential. The file is human-editable and dotfile-manageable.

Current schema:

```json
{
  "checkout_dir": "~/src/repo-name",
  "worktree_prefix": "~/src/work-",
  "env_files": ["/absolute/path/.env", "/absolute/path/.env.local"],
  "hidden": { "snooze_until": 1717372800000 }
}
```

- **`checkout_dir`**: absolute path to the local git checkout. Defaults to `~/src/<repo-name>` (no owner prefix). The source checkout `git worktree add` runs in, and the root for env file scanning.
- **`worktree_prefix`**: prefix for spawned worktree locations. The full worktree path is `<worktree_prefix><workspace>/<repo>` — a string concatenation, so the trailing `work-` is part of the directory name, not a separate path component (e.g. `~/src/work-12-add-foo/repo-name`). Absent or `null` defaults to `~/src/work-`, preserving the original behavior. Tilde-expanded. Changing it affects future spawns only; it does not move existing worktrees.
- **`env_files`**: ordered list of `.env` files to source when launching user-facing tools (VSCode, Terminal) for this repo. Populated via a "Scan" action that walks the checkout directory (up to 4 levels, skipping `node_modules`, `.git`, `target`, etc.) looking for files whose name starts with `.env`. Users can also add or remove entries manually.
- **`hidden`**: repo-level hide/snooze state. Absent (or `null`) means visible. When present, the repo and all its work items are hidden from the dashboard unless "Show hidden" is toggled on. `snooze_until` is a Unix-epoch-millis timestamp the repo stays hidden until (`null` = hidden indefinitely); once that time passes the repo renders as visible again. Per-work-item hide state is **not** stored here — it lives on each session record (`~/.maiestro/sessions/<id>.json`).

### Global app settings

App-wide settings that are neither a credential nor repo-scoped live in `~/.maiestro/settings.json` (`backend/src/app_settings.rs`), a sibling of `profiles.json`. Like the other `~/.maiestro/` files it is human-editable and dotfile-manageable; every field is optional, so a missing or partial file falls back to defaults.

Current schema:

```json
{
  "window": { "width": 680, "height": 460 }
}
```

- **`window`**: persisted size (logical pixels) of the menu-bar popover (the `main` window). The popover is undecorated/transparent and `resizable: true`; the user drags invisible edge/corner grips (`ResizeGrips` in the frontend, forwarding to Tauri's `startResizeDragging`). The backend restores this size in `setup()` *before* the first show (so there's no resize flash) and saves the current size in the blur handler when the popover hides — a low-churn save point versus writing on every drag frame. A restored size is clamped to the `minWidth`/`minHeight` in `tauri.conf.json` (mirrored as `MIN_POPOVER_*` constants in `main.rs`). Absent `window` means "use the `tauri.conf.json` default size." Window **position** is not persisted — the popover is always re-centered under the tray icon.
