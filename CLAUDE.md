# mAIestro

## Shell commands

Never hardcode `/Users/<name>/` in Bash commands. Use relative paths or `~` instead (e.g. `~/src/maiestro`, `./backend`). A hook blocks any command containing a hardcoded `/Users/` path.

A Tauri v2 menu-bar app on macOS that launches and manages git-worktree-per-issue development workspaces, each running its own scoped Claude Code session.

Directory layout: `backend/` (Rust/Tauri) and `frontend/` (web). Not Tauri's defaults of `src-tauri/` and `src/`.

## Architectural decisions

### Conversation lives in the popover; VSCode is opt-in

The chat with Claude is the primary surface, hosted in the mAIestro popover. It is backed by a `claude` CLI subprocess running headlessly in the worktree, with stdio piped to the backend. VSCode is **not** launched to start a conversation — the user clicks "Open in VSCode" only when they want an editor, and the Claude Code VSCode extension attaches to the already-running session (via the lock file / socket `claude` writes into the worktree). One conversation, multiple possible viewers.

Sessions are owned by a backend `SessionRegistry` keyed by worktree path, so they outlive popover open/close and only terminate on explicit end or app quit.

### Talk to GitHub directly, not via `gh`

mAIestro uses the GitHub REST API directly (via `octocrab` or a thin `reqwest` wrapper) with per-profile tokens read from Keychain at call time. We do **not** shell out to `gh`.

Reasons:
- `gh auth login` has a single active account, which conflicts with the multi-agent / per-profile identity model.
- At app launch the shell environment is not sourced and `$PATH` is minimal, so `gh` may not even be discoverable.
- Tokens already need to live in Keychain for the profile model; routing them through `gh` adds a layer with no benefit.

`git` itself (clone/fetch/push) still runs as a subprocess, but with `GITHUB_TOKEN` injected into the child env and a credential helper configured per worktree (see "Credential injection" below).

### Identity = AgentProfile, stored in Keychain

The unit of identity is an `AgentProfile`: name, allowed repos, capability set, and references to credentials. The credential **values** (currently the GitHub token) live in the macOS Keychain under `com.maiestro.agent.<profile-id>.<credential-kind>`. The profile JSON in `~/.maiestro/profiles.json` holds only the references. This path is intentionally developer-friendly (like `~/.ssh/`) so profiles can be inspected, edited by hand, and managed by dotfile tooling.

Keychain is chosen because it is unlocked at user login, so credentials are available even when mAIestro launches at startup (when shell profiles are not sourced and env files are unavailable).

### Credential injection at spawn time

Every subprocess mAIestro spawns into a worktree — the `claude` CLI, a shell, `git` — runs with a **clean, explicitly constructed env**, not the parent's full env. Into that env we inject only what the selected profile authorizes: `GITHUB_TOKEN`, `GIT_{AUTHOR,COMMITTER}_{NAME,EMAIL}` matching the profile's GitHub identity, and a git credential helper that hands out `GITHUB_TOKEN` over HTTPS.

We do **not** inject an `ANTHROPIC_API_KEY`. Claude authentication always comes from the `claude` CLI's own login session (read from `~/.claude/` via `$HOME`), so the spawn env must carry `$HOME` through. mAIestro does not store or manage an Anthropic credential.

This is what makes the multi-agent model real: two worktrees open at once can act as fully distinct GitHub identities with no cross-contamination, and a worktree never inherits ambient credentials the user happens to have in their shell.

### Code signing is required for releases (and for Keychain trust)

Release builds must be signed with a **Developer ID Application** certificate and notarized. Beyond distribution, this is also what makes the app's Keychain access usable: macOS binds a Keychain item's "Always Allow" decision to the app's *designated requirement*. For an ad-hoc/unsigned build that requirement is the binary's cdhash, which changes on every rebuild — so the access prompt returns each launch. A stable Developer ID signature anchors the requirement to the certificate, so the grant persists.

Dev builds (`tauri dev`) run the raw, ad-hoc-signed binary and will re-prompt on each rebuild — that is expected and accepted. Signing is **not** committed to `tauri.conf.json`; the signing identity and notarization secrets live in a gitignored `.env.release` (template: `.env.release.example`). Run `scripts/release.sh`, which sources that file, builds via `tauri build`, and verifies the signature/notarization.

### User-facing tool launches use `open -a`, not a constructed env

When the user clicks "Open in VSCode" or "Open Terminal", mAIestro delegates to macOS Launch Services via `open -a <App> <worktree-path>`. The OS launches the app exactly as a Finder double-click would — the app receives the user's full environment (Homebrew PATH, shell integrations, all installed tools) with no env construction by mAIestro.

This is intentionally different from agent spawns: user-facing tools get the *user's* world; agent subprocesses get the *profile's* world.

Any per-repo settings the user needs to configure (e.g. which terminal app, which editor, workspace-level env overrides) are stored in per-repo settings — see "Per-repo settings" below.

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
