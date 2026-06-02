# mAIestro

## Shell commands

Never hardcode `/Users/<name>/` in Bash commands. Use relative paths or `~` instead (e.g. `~/src/maiestro`, `./backend`). A hook blocks any command containing a hardcoded `/Users/` path.

A Tauri v2 menu-bar app on macOS that launches git-worktree-per-issue development workspaces, opening a Claude Code session in VSCode or a terminal for each. mAIestro is a launcher and dashboard, not a session host.

Directory layout: `backend/` (Rust/Tauri) and `frontend/` (web). Not Tauri's defaults of `src-tauri/` and `src/`.

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

### Per-repo settings

Each repo tracked by mAIestro has a small settings record stored in `~/.maiestro/repos/<owner>-<name>.json`. This is the place for configuration that is specific to a repo but not a credential. The file is human-editable and dotfile-manageable.

Current schema:

```json
{
  "checkout_dir": "~/src/repo-name",
  "env_files": ["/absolute/path/.env", "/absolute/path/.env.local"]
}
```

- **`checkout_dir`**: absolute path to the local git checkout. Defaults to `~/src/<repo-name>` (no owner prefix). Used as the root for worktree creation and for env file scanning.
- **`env_files`**: ordered list of `.env` files to source when launching user-facing tools (VSCode, Terminal) for this repo. Populated via a "Scan" action that walks the checkout directory (up to 4 levels, skipping `node_modules`, `.git`, `target`, etc.) looking for files whose name starts with `.env`. Users can also add or remove entries manually.
