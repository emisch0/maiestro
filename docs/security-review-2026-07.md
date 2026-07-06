# Security Review — mAIestro (July 2026)

Scope: the full backend (`backend/src`), Tauri configuration and capabilities, the
frontend (`frontend/src`), and the dependency lockfiles. This addresses issue #79:
credential/token security, subprocess/command execution, file I/O and permissions,
input validation, logging, and dependencies.

The review was conducted across four focused passes (credentials/logging,
subprocess/injection, file-I/O/validation, Tauri/frontend) plus dependency scans
(`cargo audit`, `pnpm audit`).

## Threat model

mAIestro is a single-user macOS menu-bar app. It holds GitHub tokens in the
Keychain, renders **remote-influenced content** (GitHub issue/PR titles and bodies)
and **session-influenced content** (Claude Code hook payloads, tool stderr) in its
webviews, and executes local subprocesses (git, editors, `post_spawn_commands`).
The relevant adversaries are therefore: (a) a malicious or compromised GitHub repo /
issue whose text flows into paths, shell strings, or the UI; (b) another local
process running as the same user abusing the hook CLI or status directory; and (c)
a hypothetical XSS foothold in the webview trying to reach the token or code
execution. "The user edits their own `~/.maiestro` config to hurt themselves" is not
a real adversary — those files are documented as human-editable.

Overall posture is **strong**. Subprocess execution is almost entirely
`execve`-style (`Command::new(...).args(...)`, no shell), the two unavoidable shell
strings use a correct POSIX single-quote escaper, GitHub issue text is slugified
before it can reach a path, tokens live only in the Keychain and only ever appear in
an `Authorization` header that is never logged, and React escapes all rendered text
(no `dangerouslySetInnerHTML` anywhere). The findings below are hardening of a
sound design; none was an actively-exploitable critical bug.

## Fixes applied in this change

| # | Severity | Finding | Fix |
|---|----------|---------|-----|
| 1 | Medium | `--workspace` / `clear_session_error` workspace id was used verbatim in a status-file path — `..`/`/` could escape `~/.maiestro/status/` and write attacker-influenced JSON to an arbitrary user-writable path | `is_safe_workspace_id` guard at both entry points and defensively in `write_record_atomic` (`backend/src/status.rs`) |
| 2 | Medium | `credentials_get` returned the **raw GitHub token** across the IPC boundary, though the frontend only used it as a boolean "is it set?" probe — any webview foothold could harvest every identity's token | Replaced with `credentials_exists` returning `bool`; removed `credentials_get` from the invoke handler; frontend uses the boolean (`backend/src/credentials.rs`, `backend/src/main.rs`, `frontend/src/api.ts`, `frontend/src/App.tsx`) |
| 3 | Medium | No Content-Security-Policy (`csp: null`) while rendering remote GitHub content — any future injection sink would have no second layer | Strict CSP: `script-src 'self'`, `object-src 'none'`, `base-uri 'self'`, `frame-src 'none'`, tight `connect-src` (`backend/tauri.conf.json`) |
| 4 | Medium | `env_files` entries (repo settings) were joined onto the checkout/worktree with no containment — an absolute or `..` entry could copy an arbitrary file *into* the worktree or write the copy *outside* it | `is_contained_relpath` check rejects absolute / `..` entries before copying (`backend/src/spawn.rs`) |
| 5 | Low/Med | Teardown's `remove_dir_all` guard was a raw string prefix that a `..`-laden `work_dir` (from a tampered session record) could tunnel through while still matching the prefix literally | Reject any `..` component before the prefix check (`backend/src/spawn.rs`) |
| 6 | Low | Tool-failure error text (e.g. a failed `curl https://user:token@host/…`) was written verbatim to the persistent log and the popover — a credentialed URL could leak | `redact_secrets` masks URL `user:pass@` userinfo and bounds length, applied at the single `extract_error_message` choke point (`backend/src/status.rs`) |
| 7 | Low | AppleScript window markers stripped only `"`, not `\` — a trailing backslash could break out of the string literal | Strip both `"` and `\` via `applescript_literal_safe` (`backend/src/spawn.rs`) |
| 8 | High (dep) | `quick-xml` ≤0.40 DoS advisories (RUSTSEC-2026-0194/0195), pulled transitively via `tauri` → `plist` | `cargo update -p plist -p quick-xml` → plist 1.10.0, quick-xml 0.41.0 |
| 9 | High (dep) | `vite` ≤6.4.2 advisories (GHSA-fx2h-pf6j-xcff, GHSA-v6wh-96g9-6wx3) | Bumped to `^6.4.3`; `pnpm audit` now clean |

All new guards have unit tests (`status.rs`: `workspace_id_validation`,
`redact_masks_url_userinfo`). `cargo test` (21 passing) and `pnpm build` (tsc +
vite) both pass after the changes.

## Verified sound (no change needed)

- **Tokens never logged.** The only token read sites are `credentials_exists` and
  `GitHub::for_identity`; `GitHub::send` logs method + URL + status only, and the
  token lives solely in the bearer header — never in a URL, so even `RUST_LOG=debug`
  and reqwest error `Display` cannot leak it. No `Debug`/`Display` impl formats a
  token-bearing struct.
- **No secrets on disk.** Tokens are Keychain-only; `~/.maiestro/**` holds only repo
  names, local paths, branch names, issue metadata and (now-redacted) error text.
- **Deletion lifecycle.** `identities_remove` deletes the Keychain item before
  dropping the identity and aborts on a real Keychain error — no orphaned tokens.
- **Subprocess injection.** All git/editor/`open` launches are `execve`-style. The
  two shell strings (Claude hook command, VS Code task) route user/LLM text through
  the canonical `'\''` single-quote escaper. `git` args are never attacker-shaped
  into option injection (`default_branch` is always `origin/`-prefixed; branch names
  come from `slugify`, which emits only `[a-z0-9-]`).
- **Path traversal from issue titles.** `slugify` maps `/`, `.`, `..` and all
  Unicode to collapsed `-`, so a hostile issue title cannot escape the worktree path.
- **`post_spawn_commands`** are arbitrary shell *by design*, but sourced only from
  user-authored repo settings — never from cloned repo contents, the issue, or the
  LLM — run in the correct cwd with a per-command timeout.
- **Claude drafting** runs with `--tools ""`, containing prompt-injection from issue
  text to at worst a bad draft the user reviews.
- **JSON parsing** of GitHub responses and hook stdin is panic-free
  (`unwrap_or`-style), with a cycle guard and pagination cap on issue-tree building.
- **Repo-settings load** validates against the embedded JSON Schema and fails loud
  rather than silently falling back to defaults; a drift test keeps schema and
  struct aligned.
- **Frontend rendering.** No `dangerouslySetInnerHTML` / `innerHTML` anywhere; all
  GitHub text, error strings and `last_error.message` render as escaped React text.
  `open_url` enforces an `http(s)` scheme allowlist.
- **Atomic status writes** (temp file + rename) mean the watcher never reads a
  half-written record.
- **`git info/exclude` append** is idempotent and writes only fixed literals.

## Remaining recommendations (not applied — larger or lower value)

- **Per-window capability / command scoping (Low/Med).** All three windows share one
  capability set, and Tauri v2 app commands aren't ACL-gated by default, so any
  window's JS can invoke any command (e.g. `spawn_work`, `repo_settings_set`). Fixing
  #2 removes the token-*exfiltration* path; an XSS foothold could still drive
  authenticated GitHub mutations or write `post_spawn_commands` + spawn. Scoping
  commands per window (opting into the command ACL) would shrink this surface but is
  a non-trivial change with regression risk — deferred.
- **Anchor `href` to GitHub `html_url` (Low).** `App.tsx` sets both `href` and an
  `onClick`→`openUrl` handler; a middle/⌘-click bypasses the backend scheme check.
  Practically safe (GitHub only emits `https://github.com/…` URLs), but dropping the
  raw `href` in favor of a button, or validating the scheme before assigning `href`,
  would remove the one DOM-navigation sink fed by remote data.
- **`open_path` (Low).** Reachable from any window and only checks existence; could
  `open` (launch) an app bundle. Exploitation needs a compromised frontend.
  Restricting to directories under a known-safe root would harden it.
- **`~/.maiestro` file permissions (Low).** Files are created with the default umask
  (world-readable). Nothing secret lives there, so impact is negligible on a personal
  Mac; creating the dir `0700` / files `0600` is optional defense in depth.
- **Prompts in `claude` argv (Info).** Draft prompts (incl. the working diff) are
  passed as argv, visible to same-user `ps`. No credentials involved; piping over
  stdin would remove the exposure.
- **Unmaintained transitive crates (Info).** `cargo audit` flags several
  `unic-*`/`proc-macro-error`/`glib` advisories as unmaintained/unsound; all are
  transitive (mostly via `tauri`) with no drop-in fix and no known exploit path here.
  Track upstream.

## Verification

```
cargo test        # 21 passed
cargo audit       # no remaining vulnerabilities (only unmaintained/unsound warnings, all transitive)
pnpm audit        # No known vulnerabilities found
pnpm build        # tsc + vite build clean
```

Note: the CSP change (#3) should be smoke-tested in both `tauri dev` and a packaged
build to confirm no asset is blocked; the policy was written to include the dev
server and IPC origins so both paths keep working.
