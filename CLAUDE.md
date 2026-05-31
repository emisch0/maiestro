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

The unit of identity is an `AgentProfile`: name, allowed repos, capability set, and references to credentials. The credential **values** (GitHub token, Anthropic key) live in the macOS Keychain under `com.maiestro.agent.<profile-id>.<credential-kind>`. The profile JSON in `~/Library/Application Support/com.maiestro/profiles.json` holds only the references.

Keychain is chosen because it is unlocked at user login, so credentials are available even when mAIestro launches at startup (when shell profiles are not sourced and env files are unavailable).

### Credential injection at spawn time

Every subprocess mAIestro spawns into a worktree — the `claude` CLI, a shell, `git` — runs with a **clean, explicitly constructed env**, not the parent's full env. Into that env we inject only what the selected profile authorizes: `GITHUB_TOKEN`, `ANTHROPIC_API_KEY`, `GIT_{AUTHOR,COMMITTER}_{NAME,EMAIL}` matching the profile's GitHub identity, and a git credential helper that hands out `GITHUB_TOKEN` over HTTPS.

This is what makes the multi-agent model real: two worktrees open at once can act as fully distinct GitHub identities with no cross-contamination, and a worktree never inherits ambient credentials the user happens to have in their shell.
