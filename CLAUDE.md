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

### All launches use `open -a`, not a constructed env

Every launch hands off to the OS rather than building an env: opening an app uses Launch Services (`open -a <App> <worktree-path>`), and starting `claude` in a standalone terminal uses the terminal's own run-command (e.g. `osascript … do script "cd <worktree> && claude …"`), which runs under the user's login shell. Either way the session inherits the user's full environment (Homebrew PATH, shell integrations, all installed tools) with no env construction by mAIestro. There is no separate "agent spawn with a constructed env" path; everything mAIestro launches gets the *user's* world.

Which app a session opens in (a specific terminal, an editor) and any workspace-level env files are stored in per-repo settings — see "Per-repo settings" below.

One deliberate exception: when VS Code is available, we open worktrees via its `code` CLI (located on `$PATH`, falling back to common install paths and the bundled `.app/Contents/Resources/app/bin/code`) instead of `open -a`, so we can pass `--disable-workspace-trust` and skip the "Do you trust the authors?" prompt on every freshly spawned worktree. The `code` CLI forwards that flag even to an already-running VS Code, which `open -a --args` cannot. If no `code` CLI is found we fall back to `open -a "Visual Studio Code" --args --disable-workspace-trust <worktree>`. The session still inherits the user's ambient environment either way.

### Live per-session status via Claude Code hooks

Because mAIestro launches `claude` but does not host it (see above), it cannot read a session's working/waiting state from stdio. Instead, at worktree creation `spawn.rs` writes Claude Code hooks into the worktree's `.claude/settings.local.json` (the personal, gitignored layer that merges with the user's own settings and applies to both terminal and VS Code integrated-terminal sessions). Each hook invokes **the mAIestro binary itself** as `maiestro hook <state> --workspace <ws-id>` — a hidden CLI subcommand dispatched in `main()` *before* Tauri starts. Using the app binary as the helper means zero external deps (no `jq`/`python`) and one source of truth; `spawn.rs` bakes its own `current_exe()` path into the generated commands.

The helper reads Claude's hook event JSON on stdin (serde), and atomically writes a status record to `~/.maiestro/status/<ws-id>.json`. The backend watches that directory with the `notify` crate and emits a `session-status` event to the popover; the frontend also reads `sessions_status_list` on open so a reopened popover is correct even if it missed events. The hook → state mapping: `SessionStart`→running, `UserPromptSubmit`/`PreToolUse`/`PostToolUse`→busy, `Notification`→needs_you (detail from the payload message), `Stop`→idle, `SessionEnd`→ended. `PostToolUse`→busy is what flips a session out of "needs you" after an approved permission prompt: the prompt's `Notification` sets needs_you, and `PostToolUse` (fired once the approved tool completes) is the only signal that Claude has resumed working — `PreToolUse` can't serve this, since it fires *before* the prompt, not after approval. The one unavoidable gap is the in-flight execution of a permission-gated tool: there is no hook at the moment of approval, so the pill reads needs_you until that tool finishes. The helper is deliberately failure-tolerant — a hook must never block or crash the user's session.

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
