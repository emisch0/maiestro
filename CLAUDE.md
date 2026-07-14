# mAIestro

## Shell commands

Never hardcode `/Users/<name>/` in Bash commands. Use relative paths or `~` instead (e.g. `~/src/maiestro`, `./backend`). A hook blocks any command containing a hardcoded `/Users/` path.

A Tauri v2 menu-bar app on macOS that launches git-worktree-per-issue development workspaces, opening a Claude Code session in VSCode or a terminal for each. mAIestro is a launcher and dashboard, not a session host.

Directory layout: `backend/` (Rust/Tauri) and `frontend/` (web). Not Tauri's defaults of `src-tauri/` and `src/`.

The frontend uses **pnpm** (`pnpm-lock.yaml`), not npm or yarn. Use `pnpm install` / `pnpm dev` — `npm install` fails on the `link:` workspace deps.

## Git workflow

**Every change to `main` must go through a pull request.** Do not commit or push directly to `main`: branch, push the branch, open a PR, and merge it on GitHub. This holds even for small fixes and the release flow.

Because the repo is private on a free plan, GitHub-side branch protection isn't available, so this is enforced locally by a committed `pre-push` hook (`.githooks/pre-push`) that rejects any push to `main`. Enable it once per clone with `git config core.hooksPath .githooks` (already set in the primary checkout; worktrees share it via the common git dir). The hook is a guardrail, not a hard wall — an emergency override is `git push --no-verify`, to be used sparingly.

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

Dev builds (`tauri dev`) run the raw, ad-hoc-signed binary and will re-prompt on each rebuild — that is expected and accepted. Signing is **not** committed to `tauri.conf.json`; the signing identity and notarization secrets live in a gitignored `.env.release` (template: `.env.release.example`).

`scripts/release.sh` is the release pipeline, split into three idempotent phases so a partially-failed release resumes by re-running it:

- **`bump <patch|minor|major|X.Y.Z>`** — version is single-sourced from `backend/tauri.conf.json`; this bumps it there plus `package.json` and `backend/Cargo.toml`, syncs `backend/Cargo.lock` (via `cargo metadata --offline`, no full build), and asserts all four agree. It does not commit.
- **`build`** — sources `.env.release`, validates the `APPLE_SIGNING_IDENTITY`, runs `pnpm tauri build` (Tauri auto-notarizes when the Apple credentials are present), and verifies the signature / Gatekeeper assessment / notarization staple.
- **`publish [--notes-file <file>]`** — reads the version, derives `owner/repo` from the `origin` remote, then tags `vX.Y.Z`, creates the GitHub Release, and uploads the notarized `.dmg`. It talks to GitHub via the **REST API** (`curl` + `GITHUB_TOKEN` from `.env.release`), not `gh`, matching the backend's "Talk to GitHub directly" decision. Each step (tag, release, asset) is skipped if already present, and it refuses to publish a dirty tree or an un-notarized `.dmg`.

The **`/release` skill** (`.claude/skills/release/SKILL.md`) drives the whole pipeline, adding the judgment steps (release-notes drafting, user confirmation). It always runs from the **primary checkout** (the first `git worktree list` entry) on `main`, so it works even when invoked from a mAIestro-spawned feature worktree — which has no `.env.release` and must never be released from.

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

One deliberate exception: when VS Code is available, we open worktrees via its `code` CLI (resolved by `tools::find_tool("code")` — see "External tool resolution") instead of `open -a`, so we can pass `--disable-workspace-trust` and skip the "Do you trust the authors?" prompt on every freshly spawned worktree. The `code` CLI forwards that flag even to an already-running VS Code, which `open -a --args` cannot. If no `code` CLI is found we fall back to `open -a "Visual Studio Code" --args --disable-workspace-trust <worktree>`. The session still inherits the user's ambient environment either way.

### External tool resolution

mAIestro shells out to a handful of external CLIs *itself* (distinct from the user-facing session it launches, which inherits the full ambient env). When the app is launched from the packaged bundle (`/Applications/mAIestro.app/…`) at login, macOS Launch Services gives it a **minimal `$PATH`** (`/usr/bin:/bin:/usr/sbin:/sbin`) with no shell profile sourced, so a bare `Command::new("claude")` (or `git`, or `code`) can fail to resolve or pick the wrong binary (e.g. the `/usr/bin/git` Xcode stub instead of a Homebrew git). `backend/src/tools.rs` centralizes resolution (issue #85).

- **Tools that go through `tools`:** `claude` (AI drafting in `claude_text`), `git` (`git()` / `local_branch_exists()` — worktree add, branch checks), and the VS Code `code` CLI (`open_vscode`). These are the ones invoked directly with the minimal PATH.
- **Tools that don't:** `open`, `osascript`, `lsof` are system binaries under `/usr/bin`, always on the minimal PATH; the login shell used for `post_spawn_commands` is invoked with `-l` so its children get a full PATH; and `gh` is **not used at all** (GitHub is the REST API — see "Talk to GitHub directly").
- **Login-shell PATH, recovered once.** At startup `tools::init()` (called from `setup()`) runs `$SHELL -l -c 'echo $PATH'` and caches the result — the user's *real* PATH (Homebrew, asdf/nvm shims, …). `enriched_path()` = that PATH followed by the process's own PATH (dedup, order-preserving), and every directly-spawned child is given it (`tools::command` / `tools::tokio_command`) so sub-tools the resolved binary calls resolve too. This one layer fixes all three tools — and any added later — with no config.
- **`resolve_tool(name)` precedence:** explicit override (`tool_paths` in global settings, if the file exists) → `which` on the enriched PATH → known install locations (`~/.claude/local/claude`, `/opt/homebrew/bin`, …) → the bare name (let the OS try, as a last resort preserving prior behavior). `find_tool` is the same without the bare-name fallback (returns `Option`); `resolved_status` reports the picked path + whether it exists, for the Settings UI. `tools_resolved` is the Tauri command backing the "Tool paths" status line.
- **Override config** lives in `~/.maiestro/settings.json`'s `tool_paths` (see "Global app settings"): a per-tool absolute path, empty = auto-resolve.

### Repo health check (prerequisites + GitHub permissions)

A **Check Health** button at the top of a repo's detail form in the Settings window runs per-repo diagnostics and shows the results in a modal, so a broken prerequisite surfaces *before* it fails mid-spawn (issue #93). The backend command is `health::repo_health_check(repo) -> HealthReport` (`backend/src/health.rs`); the frontend renders it via `api.repoHealthCheck` and a `HealthModal` in `App.tsx`. Each check is a `HealthCheck { id, label, status, detail, sub }` with `status ∈ {pass, fail, warn, skipped}`; a parent's status is the worst-case roll-up of its `sub` checks. The checks are **informational only** — they never block spawning.

The six checks: **cloned repo** (`cloned_repo_dir` exists *and* `git rev-parse --git-dir` succeeds), **git available**, **claude available** (both via `tools::find_tool`), **GitHub token & permissions** (below), **session editor available** (the `code` CLI, matching what `open_vscode` actually launches; a missing CLI with VS Code.app present is a `warn`, not a `fail`), and **env files exist** (each `env_files` entry resolved against `cloned_repo_dir` as the spawn copy does; missing ones are a `warn`).

**GitHub permission testing is the notable one, and it never mutates the repo.** mAIestro's own GitHub calls need read (repo/issues/PRs) *and* write (create/update issues, comments, assignees, create/merge PRs), so token validity alone isn't enough — but there's no side-effect-free "test this write endpoint" call. So write access is **derived, not exercised** (no throwaway issue/PR/label):
- **Token valid** — `GitHub::check_token()` does a raw (non-ETag-cached) `GET /user`, returning the login *and* the `X-OAuth-Scopes` response header. This is the only addition to `plugins/github.rs`.
- **Repo readable** — `GitHub::repo()` (`GET /repos/{owner}/{name}`), which also carries the `permissions` object.
- **Write access** — for a **classic PAT** (non-empty `X-OAuth-Scopes`) the scope list is authoritative: needs `repo` (or `public_repo` on a public repo). For a **fine-grained PAT** (empty scopes header) it reads the repo response's `permissions.push` flag. Either way the verdict ("token can read but cannot push to this repo") is inferred from what GitHub already reports.

The check uses the repo's assigned `identity_id` to pick the token (`GitHub::for_identity`); a repo with no identity assigned makes the GitHub check `skipped`. Out of scope for now: no live write probe, no `gh` CLI check, no auto-fix, and no background/periodic polling (runs only on button click).

### Live per-session status via Claude Code hooks

Because mAIestro launches `claude` but does not host it (see above), it cannot read a session's working/waiting state from stdio. Instead, at worktree creation `spawn.rs` writes Claude Code hooks into the worktree's `.claude/settings.local.json` (the personal, gitignored layer that merges with the user's own settings and applies to both terminal and VS Code integrated-terminal sessions). Each hook invokes **the mAIestro binary itself** as `maiestro hook <state> --workspace <ws-id>` — a hidden CLI subcommand dispatched in `main()` *before* Tauri starts. Using the app binary as the helper means zero external deps (no `jq`/`python`) and one source of truth; `spawn.rs` bakes its own `current_exe()` path into the generated commands.

That baked path only survives while the spawning build does, so a worktree spawned by a `tauri dev` build (ephemeral `current_exe()`) or a feature worktree that later gets torn down/merged would point its hooks at a missing binary, erroring on every event. To self-heal, **at startup mAIestro reconciles every tracked session's hooks** (`spawn::reconcile_all_session_hooks`, called from `setup()` right after `status::sweep_stale`): for each session it rewrites any of *our* hook commands in `.claude/settings.local.json` to the currently-running binary's path, identifying ours by the trailing `--workspace '<ws-id>'` so unrelated hooks are untouched, only when the worktree already carries our hooks, and writing only when the path actually changed. The reuse-existing-workspace spawn path reconciles too, so reopening heals without a restart. Release builds resolve to the stable `/Applications/mAIestro.app/.../maiestro` and stay fixed; dev builds re-point on each launch. The remaining gap is purely between a spawner disappearing and the next mAIestro launch.

The helper reads Claude's hook event JSON on stdin (serde), and atomically writes a status record to `~/.maiestro/status/<ws-id>.json`. The backend watches that directory with the `notify` crate and emits a `session-status` event to the popover; the frontend also reads `sessions_status_list` on open so a reopened popover is correct even if it missed events. The hook → state mapping: `SessionStart`→running, `UserPromptSubmit`/`PreToolUse`/`PostToolUse`/`PostToolUseFailure`→busy, `Notification`→needs_you (detail from the payload message), `Stop`→idle, `SessionEnd`→ended.

**The `creating` state (background spawn, issue #77).** A fresh spawn is two-phase. `do_spawn` runs a fast synchronous phase — resolve the final workspace id, write the `Session` record, and write a `state: "creating"` status (`status::write_creating`) — then hands the slow work (worktree add, env copy, GitHub assign/comment, `post_spawn_commands`, editor launch) to a background `tokio::spawn` (`finish_spawn`) and returns at once. So `confirm_spawn` resolves quickly, the popover lands on the dashboard immediately, and the new row shows a "Creating…" `workspace-op-pill` (the same monochrome busy-ring pill as "Merging…"/"Tearing down…", not an AI/Claude status pill) until the worktree is ready. `creating` is mAIestro's own pre-Claude state, not a hook state: `finish_spawn` clears it (`status::clear_creating`, which only removes a still-`creating` record) right before opening the editor, handing the status over to Claude's own hooks; on a fatal failure it instead writes a surfaced `last_error` (`status::write_spawn_error`) so the broken row shows the usual dismissible error block and can be torn down (teardown tolerates a never-created worktree). Reopening an existing worktree stays fully synchronous — no `creating` pill. `PostToolUse`→busy is what flips a session out of "needs you" after an approved permission prompt: the prompt's `Notification` sets needs_you, and `PostToolUse` (fired once the approved tool completes) is the only signal that Claude has resumed working — `PreToolUse` can't serve this, since it fires *before* the prompt, not after approval. The one unavoidable gap is the in-flight execution of a permission-gated tool: there is no hook at the moment of approval, so the pill reads needs_you until that tool finishes. The helper is deliberately failure-tolerant — a hook must never block or crash the user's session.

**Failed tool calls.** `PostToolUseFailure` fires after a tool call fails. The state stays `busy` (Claude works on past the failure), but the helper also captures the error into a separate `last_error` field on the status record (`{ tool, message, ts, count, surfaced }`) and logs it to the standard backend log (`logging::append_line` → today's `~/Library/Logs/com.maiestro.app/…` file; the hook subprocess writes there directly since it's too short-lived to install the `tracing` subscriber). Because the status file is last-write-wins and the next hook lands within seconds, `last_error` can't live in `state`: instead the helper does a **read-merge-write**.

Crucially, **not every failure is shown.** Claude often recovers from a transient failure (a flaky network tool, a timeout it retries) and keeps working, so surfacing each one immediately is noise (issue #48). Every failure is *logged*, but the popover only displays `last_error` once `surfaced` is true. The lifecycle:
- `PostToolUseFailure`→`tool_failed` *sets* it as a **pending** (`surfaced: false`) error. `count` tracks consecutive failures of the *same* tool; a repeated failure (`count >= RETRY_SURFACE_THRESHOLD`, currently 2) is persistent and surfaces immediately.
- `PostToolUse`→`tool_ok` (a tool *succeeded*) *clears* a pending error — Claude recovered. An already-surfaced error stays until dismissed. This is why `PostToolUse` maps through its own `tool_ok` verb rather than sharing `busy` with `PreToolUse`: only `tool_ok` means a tool finished successfully, the signal that distinguishes recovery from a retry attempt.
- `Stop`→`idle` *promotes* a pending error to `surfaced: true` — Claude stopped without recovering, so the failed call was effectively its last action and the user should see it.
- A new turn or session (`UserPromptSubmit`→`prompt`, `SessionStart`→`running`) *clears* it. (This is why `UserPromptSubmit` maps through its own `prompt` verb rather than sharing `busy`.)
- Every other event *carries it forward*.

The popover shows a *surfaced* `last_error` as a dismissible inline block (the same `.cleanup-confirm` pattern PR-create errors use, never a tooltip) and tints the Claude pill red; a pending one stays hidden. The **Dismiss** button calls the `clear_session_error` command, which rewrites the record without `last_error` so it doesn't reappear on reopen. The exact error field in the `PostToolUseFailure` payload isn't pinned down in the docs, so `extract_error_message` reads several likely fields in order and falls back to a generic message. Because `reconcile_all_session_hooks` rebuilds hook commands from `maiestro_hook_groups` on every startup, the `PostToolUse`→`tool_ok` verb change reaches already-spawned worktrees on the next app launch, no re-spawn needed.

The whole mechanism is event-driven and last-write-wins, assuming one session per worktree (keyed by `<ws-id>`, which equals `Session.id`). Generated files (`.claude/settings.local.json`, `.vscode/`) are added to the worktree's shared git exclude (`$(git rev-parse --git-common-dir)/info/exclude`) so they don't trip teardown's `git status --porcelain` dirty check before Claude has run.

### Per-repo settings

Each repo tracked by mAIestro has a small settings record stored in `~/.maiestro/repos/<owner>-<name>.json`. This is the place for configuration that is specific to a repo but not a credential. The file is human-editable and dotfile-manageable.

Current schema:

```json
{
  "cloned_repo_dir": "~/src/repo-name",
  "worktree_prefix": "~/src/work-",
  "env_files": ["/absolute/path/.env", "/absolute/path/.env.local"],
  "post_spawn_commands": ["pnpm install"],
  "prompt_model": "haiku",
  "hidden": { "snooze_until": 1717372800000 },
  "prompts": { "draft_issue": null, "short_label": null, "draft_pr": null }
}
```

- **`cloned_repo_dir`**: absolute path to the local git clone (the primary checkout). Defaults to `~/src/<repo-name>` (no owner prefix). The source clone `git worktree add` runs in, and the root for env file scanning. (Renamed from the legacy `checkout_dir` key — issue #91.)
- **`worktree_prefix`**: prefix for spawned worktree locations. The full worktree path is `<worktree_prefix><workspace>/<repo>` — a string concatenation, so the trailing `work-` is part of the directory name, not a separate path component (e.g. `~/src/work-12-add-foo/repo-name`). Absent/`null`/empty falls back to the schema `default` (`~/src/work-`). Tilde-expanded. Changing it affects future spawns only; it does not move existing worktrees.
- **`env_files`**: ordered list of `.env` files to source when launching user-facing tools (VSCode, Terminal) for this repo. Populated via a "Scan" action that walks the cloned repo directory (up to 4 levels, skipping `node_modules`, `.git`, `target`, etc.) looking for files whose name starts with `.env`. Users can also add or remove entries manually.
- **`post_spawn_commands`**: ordered list of shell commands run in a **freshly-created** worktree right after `git worktree add` (and env-file copy / hook setup), before the editor opens — e.g. `pnpm install`. Empty by default. Run by `run_post_spawn_commands` in `spawn.rs` via the user's login shell (`$SHELL -lc`) so PATH and tool managers (nvm/pnpm/asdf) are available (mAIestro's own env is minimal). They run in order, stop at the first failure or a 10-minute per-command timeout, and surface a `SpawnResult.warnings` entry on a problem — the worktree is never torn down for a command failure. The reuse-existing-worktree spawn path does **not** run them (nothing new was created).
- **`prompt_model`**: which Claude model runs mAIestro's **own** programmatic prompts (`draft_issue`, `short_label`, `draft_pr`) via the headless `claude -p` calls in `spawn.rs`. `null`/empty = the schema default (`haiku`); resolved by `prompts::model(...)` and threaded through `claude_text`'s single `--model` choke point. The value is passed **verbatim** to `claude --model`, so any value that flag accepts works — a tier alias (`haiku`, `sonnet`, `opus`, `fable`) or a full model id (`claude-haiku-4-5`). It is deliberately **not** a closed schema enum: an alias auto-tracks the latest model in its tier (so `haiku` always resolves to the current fastest tier), and a newly released tier can be selected without a mAIestro update. The Settings form renders a combobox that *suggests* the common aliases (`PROMPT_MODEL_SUGGESTIONS`) but accepts any typed value; a bad value fails at draft time (surfaced, non-fatal) exactly like a bad override prompt. Applies **only** to these drafting calls — never to the user-facing session launched in the worktree, which uses the user's ambient `claude` config.
- **`hidden`**: repo-level hide/snooze state. Absent (or `null`) means visible. When present, the repo and all its work items are hidden from the dashboard unless "Show hidden" is toggled on. `snooze_until` is a Unix-epoch-millis timestamp the repo stays hidden until (`null` = hidden indefinitely); once that time passes the repo renders as visible again. Per-work-item hide state is **not** stored here — it lives on each session record (`~/.maiestro/sessions/<id>.json`).
- **`prompts`**: per-repo overrides for the AI prompt instructions mAIestro sends to Claude (`backend/src/prompts.rs`). Three fields, each `null`/empty = use the built-in default: `draft_issue` (idea → GitHub issue), `short_label` (issue → short workspace label), `draft_pr` (diff → PR description). Each prompt is assembled as `<instruction> + <runtime context>`: the **instruction** is what these overrides (or the defaults) supply; the **runtime context** (your idea, the issue title/body, the diff) is appended by `spawn.rs` and is *not* configurable, so an override — even arbitrary text or a `/skill` invocation — can't drop the data the draft needs. The Settings form shows each prompt's default text in a paragraph (textarea) field for editing.

**Defaults live in the JSON Schema only.** The `worktree_prefix`, `prompt_model`, and `prompts.*` defaults are declared as JSON Schema `default` keywords in `repo-settings.schema.json` and read from there by both sides — the backend via `repo_settings::schema_default("/properties/.../default")` (JSON Pointer into the embedded schema), and the Settings form via `extractFormDefaults` (the form strips `default` before handing the schema to JsonForms so it doesn't auto-inject them into the null-means-use-default fields). There is no hardcoded copy of these defaults in Rust or TS.

**Soft path validation + Finder reveal (issue #88).** Every path-referencing field in the Settings window (`cloned_repo_dir`, `worktree_prefix`, each `env_files` entry; and `tool_paths` in global settings) gets a light-touch existence check and a folder-icon **reveal-in-Finder** button, via the shared `frontend/src/PathField.tsx` (the `usePathExists` hook, `RevealButton`, `PathMissingHint`). The check is **advisory only** — it debounces a call to the `path_exists` command and shows an inline "not found" hint, but does *not* feed ajv/schema validation, so it never blocks the debounced autosave. The reveal button (disabled when the path is empty or missing) calls `reveal_path`, which is `open -R` (reveal-and-select, so an env *file* is selected rather than launched) — distinct from `open_path`'s bare `open`. Both commands live in `links.rs` and tilde-expand via the shared `paths::expand_tilde` (promoted out of `tools.rs`/`spawn.rs`, which had duplicate copies). Two fields aren't validated as-is because they aren't standalone paths: `worktree_prefix` is a string prefix (never a real path), so its check/reveal targets the **parent directory** (`~/src` for `~/src/work-`); and `env_files` entries are **relative to `cloned_repo_dir`** (the backend copies them from `cloned_repo.join(rel)`), so each is resolved against the repo's cloned repo dir before checking. Both resolutions are computed frontend-side.

#### Schema and validation

The file format has a hand-written JSON Schema, checked in at `backend/schemas/repo-settings.schema.json`. It is the **spec** for the format — deliberately *not* generated from the Rust `RepoSettings`/`HideState` structs. We chose a stable, human-authored artifact (with its own field descriptions) over `schemars`-style generation; the drift risk generation would have eliminated is instead caught by a test. `backend/src/repo_settings.rs` embeds the schema with `include_str!`, and the `repo_settings_schema` Tauri command returns it to the Settings window's JSON Forms renderer.

- **Drift guard (`schema_matches_struct` test).** A unit test serializes a default and a fully-populated `RepoSettings`, asserts both validate against the embedded schema, and asserts the schema's top-level `properties` key-set equals the serialized field-set. Adding a field to the struct *or* the schema without the other fails `cargo test`. This is what keeps the hand-written schema honest.
- **Load-time validation (`load_validated`).** Reading a settings file now validates it (via the `jsonschema` crate) before deserializing, with three outcomes: **missing** → defaults (unchanged); **invalid** (bad JSON or a schema violation) → a loud `Err` naming the file and the failing field; **valid** → the parsed settings. `repo_settings_get` is therefore fallible (`Result<RepoSettings, String>`) and its ~13 `spawn.rs` call sites propagate the error, so a spawn fails loudly rather than silently running with a wrong `cloned_repo_dir`. `repo_set_visibility` errors instead of clobbering an unparseable file with defaults, and `repo_settings_set` validates the value before writing (defense in depth). The Settings window shows a load failure as a banner instead of a form full of defaults. `repos_list` stays lenient (a single malformed file shouldn't drop the repo from the list).
- **Unknown fields are tolerated** (`additionalProperties` is not `false`), so a file written by a newer app version still loads in an older one, and a hand-edited `$schema` key is allowed.
- **Editor autocomplete.** The schema ships as a Tauri bundle resource at `mAIestro.app/Contents/Resources/schemas/repo-settings.schema.json`, so a hand-editor can point a `$schema` key at it for autocomplete in VS Code (for an installed app: `/Applications/mAIestro.app/Contents/Resources/schemas/repo-settings.schema.json`). It is a build artifact versioned with the binary — the app deliberately does **not** write it into `~/.maiestro/`, which holds only user-editable configuration. In a dev checkout, point `$schema` at `backend/schemas/repo-settings.schema.json` directly.

The Settings window's repo detail form is rendered by **JSON Forms** (`@jsonforms/{core,react,vanilla-renderers}`) from the fetched schema plus a hand-written UI schema (`frontend/src/RepoSettingsForm.tsx`) that orders the fields and hides `repo`/`hidden`. Two custom renderers cover what the schema can't express: an **identity select** (options from the live identity list) and an **env-files** list (Scan/Add/Remove). The frontend strips `$schema`/`$id` before handing the schema to JSON Forms so its bundled draft-07 ajv compiles cleanly; the backend still validates with the real draft 2020-12. Saving is debounced autosave-on-change, blocked while ajv reports errors.

### Global app settings

App-wide settings that are neither a credential nor repo-scoped live in `~/.maiestro/settings.json` (`backend/src/app_settings.rs`), a sibling of `profiles.json`. Like the other `~/.maiestro/` files it is human-editable and dotfile-manageable; every field is optional, so a missing or partial file falls back to defaults.

Current schema:

```json
{
  "window": { "width": 680, "height": 460 },
  "theme": "system",
  "tool_paths": { "claude": null, "git": null, "code": null }
}
```

Like per-repo settings, the file format has a **hand-written JSON Schema** (`backend/schemas/app-settings.schema.json`), embedded via `include_str!` in `app_settings.rs` and returned by the `app_settings_schema` command. A `schema_matches_struct` drift-guard test keeps the schema and the `AppSettings`/`ToolPaths` structs in sync, and `load_validated` surfaces a corrupt file as a loud error (a banner in the Settings window) instead of silent defaults. Unknown fields are tolerated for forward-compat. The **Preferences** panel in the Settings window renders this schema with **JSON Forms** (`frontend/src/AppSettingsForm.tsx` — UI schema + a custom theme renderer and a custom tool-paths renderer), autosaving on change like the repo form.

- **`window`**: persisted size (logical pixels) of the menu-bar popover (the `main` window). The popover is undecorated/transparent and `resizable: true`; the user drags invisible edge/corner grips (`ResizeGrips` in the frontend, forwarding to Tauri's `startResizeDragging`). The backend restores this size in `setup()` *before* the first show (so there's no resize flash) and saves the current size in the blur handler when the popover hides — a low-churn save point versus writing on every drag frame. A restored size is clamped to the `minWidth`/`minHeight` in `tauri.conf.json` (mirrored as `MIN_POPOVER_*` constants in `main.rs`). Absent `window` means "use the `tauri.conf.json` default size." Window **position** is not persisted — on each show `position_popover` (`main.rs`) anchors the popover under the tray icon (using a tray rect cached from tray events) and clamps it into the monitor's bounds so it always shows fully on-screen, never cropped when the tray icon sits near the far-right edge or after the popover has been resized wider. If the persisted size is larger than the display (a stale/huge saved size, or a small monitor), the popover is first **shrunk to fit** the monitor so it can't overflow. The placement is computed analytically (tray rect + window size + monitor bounds, all physical px) and applied in one `set_position` before `show()`, rather than read back from the live window position — that readback lags a cycle on macOS and made the placement toggle between opens; the monitor is found by testing rects directly (`monitor_at`), avoiding `monitor_from_point`'s Retina `None`. Machine-managed: modelled in the schema but hidden from the JSON-Forms UI (like `settings_window`).
- **`theme`**: chosen UI appearance — `"light"`, `"dark"`, or `"system"` (issue #12). Absent means `"system"`. Set via the **Preferences** panel's theme control, read on launch by every window (`app_settings_get_theme`, `frontend/src/theme.ts`). Each webview applies the theme to its own `<html>`: an explicit choice sets `data-theme="light|dark"`, while `"system"` removes the attribute so the `prefers-color-scheme` media query in `styles.css` follows the macOS appearance live. Both `app_settings_set_theme` (legacy) and the unified `app_settings_set` broadcast a `theme-changed` event so the popover, Settings, and Logs windows re-apply without a restart. `app_settings_set` load-merges — the form owns `theme`/`tool_paths`, while the machine-managed window sizes are kept from disk, so persisting one never clobbers the other.
- **`tool_paths`**: per-tool CLI path overrides — `claude`, `git`, `code` (issue #85). Each `null`/empty = auto-resolve; a value pins that binary. Read by `tools::resolve_tool` (see "External tool resolution"). Edited in the **Preferences** panel, where each field shows the auto-resolved path as a placeholder and a live status line (`tools_resolved`) beneath it.
