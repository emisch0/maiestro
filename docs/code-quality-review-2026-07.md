# Code Quality Review — mAIestro (July 2026)

Scope: the full backend (`backend/src`, ~6,400 lines of Rust), the frontend
(`frontend/src`, ~4,100 lines of TS/React), the frontend↔backend API contract,
configuration/schemas, documentation accuracy, and tooling/dependencies. This
addresses issue #96. Security was covered separately by the July 2026 security
review (#79, `security-review-2026-07.md`) and is not re-examined here.

The review ran as five focused passes: backend correctness, backend structure &
consistency (including the logging conventions documented in CLAUDE.md),
frontend, cross-cutting contract/docs/schema drift, and a tooling/dependency
baseline (`cargo clippy`, `cargo audit`, `pnpm audit`, `cargo test`, `tsc`).
Low-risk fixes were applied in this change (listed below); larger findings are
filed as follow-up issues or recorded as recommendations.

## Baseline (before fixes)

- `cargo clippy --all-targets`: **0 warnings**.
- `cargo test`: 41/41 pass.
- `cargo audit`: no vulnerabilities; warnings only — `anyhow` 1.0.102
  (RUSTSEC-2026-0190, unsound `Error::downcast_mut`, fixed in 1.0.103) and
  transitive unsound/unmaintained advisories (`glib` 0.18.5 via the Tauri/Linux
  dependency tree, not compiled on macOS builds).
- `pnpm audit`: clean. `tsc`/`vite build`: clean, with a 500 kB chunk-size
  warning (JSON Forms dominates the bundle).
- Dependencies are only patch/minor behind (`cargo update --dry-run`); no
  neglected majors.

## Fixes applied in this change

**Correctness**

- `spawn.rs` (teardown): the worktree **wrapper dir removal could delete a
  sibling repo's live worktree**. The wrapper `<prefix><issue>-<slug>` is keyed
  by issue number + slug only — two tracked repos with an identically-slugged
  issue share one wrapper, and teardown ran `remove_dir_all` on it
  unconditionally, destroying the other repo's worktree (including uncommitted
  work). Now the wrapper is only removed once it is empty. *(High — the one
  behavior-changing fix in this PR, applied because it is a two-line guard
  against data loss.)*
- CLAUDE.md/schema: the env-file **Scan matches only the exact name `.env`**,
  but the docs claimed "files whose name starts with `.env`" — a doc/code
  mismatch. Exact-match is the intended behavior (variants like `.env.local`
  are added manually), so the docs were corrected to match the code, not the
  other way round.
- `repo_settings.rs`: `repo_scan_env_files` didn't tilde-expand
  `cloned_repo_dir` (every other consumer does), so a `~/src/...` value made
  Scan silently return nothing. Now expanded.
- `repo_settings.rs`: `env_files` was the only non-`Option` field without
  `#[serde(default)]`, so a schema-valid hand-written file omitting it failed
  to load — blocking spawn/teardown for that repo. Fixed + regression test
  (`schema_valid_file_without_env_files_loads`).
- `spawn.rs` (preview): the Review & Spawn overlay hardcoded `~/src/work-` in
  the Worktree line, showing the wrong path for any repo with a custom
  `worktree_prefix` (and re-hardcoding a schema-only default). `SpawnPlan` now
  carries the effective `worktree_prefix` and the preview renders it.
- `links.rs`: `open_path` now tilde-expands like its siblings
  (`path_exists`/`reveal_path`), matching what CLAUDE.md already claimed.
- `App.tsx`: `CredRows` was defined *inside* `Settings`' render, so each
  keystroke in the token field remounted the subtree and **dropped input
  focus**. Hoisted to a module-level component with props.
- `App.tsx`: `openStartWork` used unguarded `setPicker(...)` — a slow issues
  fetch could resurrect an overlay the user had closed, or clobber a picker
  opened for another repo. Now guarded functional updates, matching
  `refreshIssues`.
- `App.tsx`: `handleSelectRepo` wrote the chosen identity *after* triggering
  the detail-form load; the racing load seeded the form with `identity_id:
  null` and the next autosave clobbered the assignment. Now writes before
  selecting, with the failure surfaced instead of an unhandled rejection.
- `App.tsx`: the credential-exists effect depended only on `scopeKey` but read
  `credTypes` from a stale closure — when the mount-time fetches resolved in
  the wrong order, real tokens showed "not set" until re-navigation.
  `credTypes` added to the deps.

**Robustness**

- `plugins/github.rs`: the shared reqwest client had **no timeout** — a
  black-holed connection stalled any GitHub-touching command indefinitely. Now
  30 s.
- `spawn.rs`/`health.rs`: the three timeout-bounded child processes
  (`claude_text` 90 s, `post_spawn_commands` 600 s, health's claude probe 60 s)
  never killed the child on timeout (`kill_on_drop` defaults to false), leaving
  e.g. a hung `pnpm install` mutating the worktree under the live session. Now
  `kill_on_drop(true)`.

**Logging conventions** (violations of CLAUDE.md's documented rules)

- `plugins/github.rs`: `mark_ready` and `merge_pull` — the two highest-stakes
  mutations — bypassed the `GitHub::send` choke point, so a PR merge produced
  no log line. Now routed through `send`.
- `spawn.rs`: `session_merge_pr` had no `log_invoke!` and no
  `#[tracing::instrument]` `session=` span (its peers `teardown` /
  `session_create_pr` have both); `session_pr_checks` had no
  `log_invoke_debug!`. Both added.
- `status.rs`: `sessions_status_list` (polled read → debug) and
  `clear_session_error` (user action → info) logged nothing. Both added.

**Dead code**

- Removed the unused `env_var` field from `CredentialTypeInfo` /
  `CredentialTypeDto` / the TS type — a leftover of the abandoned env-injection
  design.
- Removed the legacy `app_settings_set_theme` command (superseded by
  `app_settings_set`) and the dead frontend wrappers `spawnWork`, `setTheme`,
  `createIssue`, `createIssueAndSpawn`, `setDefaultIdentity`, plus the orphaned
  `CreateAndSpawnOutcome` type.

**API contract (TS types now describe the wire shape)**

- `RepoSettings` gains `repo` + `prompt_model` (previously silently dropped by
  any literal-constructing caller), `Session` gains `default_branch`,
  `AppSettings`/`ToolPaths` fields are now optional (serde omits `None` keys
  entirely — they were typed required-nullable), and `StatusRecord.state`'s doc
  comment now includes `creating`.

**Docs & dependencies**

- CLAUDE.md: rewrote the "Identity = AgentProfile" section, which described a
  model that no longer exists (no `AgentProfile`, no `~/.maiestro/profiles.json`,
  wrong Keychain naming — reality: identity names in `identities.json`,
  tokens under `com.maiestro.cred.<type-id>.identity.<identity-id>`). Corrected
  the `env_files` semantics (files are **copied into the worktree at spawn**,
  relative to `cloned_repo_dir` — not "sourced", not absolute), the per-repo
  and global settings examples, `credentials_get` → `credentials_exists`,
  documented the deliberate `logs_read` logging carve-out, and removed the
  `app_settings_set_theme` mention. Fixed the same "source" wording in the
  repo-settings schema and a stale `repo_prompt_defaults` comment in
  `RepoSettingsForm.tsx`.
- `cargo update anyhow` (1.0.102 → 1.0.103) resolves RUSTSEC-2026-0190.

## Findings not applied here

### High (filed as follow-up issues)

- **#99 — Split `spawn.rs` (~2,200 lines) into focused modules.** It contains six
  separable responsibilities — theming, Claude Code hooks, editor/AppleScript
  control, AI drafting, spawn core, teardown + PR lifecycle — and the
  convention drift found in this review (unlogged commands at the bottom of the
  file) is a direct symptom. Suggested seams and shared-helper promotions
  (`git()`, `snippet`) are in the follow-up issue. Effort: L.
- **#100 — Decompose `App.tsx` (~2,800 lines, three whole windows).** `Settings`
  (~800 lines) and `LogsView` share nothing with `MainView` and extract
  cleanly; `SessionRow` and the picker overlay are the next seams. Eight
  byte-identical or near-identical JSX/logic blocks (remove-repo confirm,
  dismissible-error pattern ×7, debounced autosave ×2, …) collapse into shared
  components/hooks (`useTauriListen`, `useDebouncedAutosave`,
  `DismissibleError`). Effort: L.
- **#101 — Move blocking calls off the async runtime and bound network git.**
  `spawn.rs::git()` is `std::process::Command` called from async commands —
  including `git push`/`fetch` with **no timeout** (a wedged SSH connection
  pins a tokio worker forever and the Merging…/Creating… pill never resolves);
  `session_work_state` runs two blocking git calls per session per poll tick;
  Keychain reads (`GitHub::for_identity`) and teardown's `lsof` also block.
  Route through `tools::tokio_command`/`spawn_blocking` and add timeouts to
  network git. Effort: M.

### Medium (recommendations)

- **Stale `creating` status is never reconciled.** `finish_spawn` runs as a
  detached `tokio::spawn`; if the app quits/crashes mid-spawn (plausible during
  a 10-minute `post_spawn_commands` run) the row shows "Creating…" forever on
  the next launch. At startup, convert a lingering `creating` record into a
  surfaced spawn error. (Folded into #101.)
- **Settings→identity→GitHub-client resolution is copy-pasted 9× in
  `spawn.rs`** (the exact "No identity assigned…" string appears 9 times, 4 of
  them also repeating the `.git`-exists check). Extract a
  `repo_context(repo) -> Result<(RepoSettings, GitHub), String>` helper. S.
- **`schema_value` + `validate_against_schema` duplicated verbatim** between
  `repo_settings.rs` and `app_settings.rs`; a shared `schema.rs` helper. S.
- **`identities_set_default` is registered but unreachable from the UI** — the
  default identity can only ever be the first-registered one. Either wire a
  "make default" control into the Settings identity panel or drop the command.
  S–M.
- **Dead backend command paths**: `spawn_work`, `create_issue`,
  `create_issue_and_spawn` are registered but no longer invoked (the UI moved
  to the preview flow); their TS wrappers were removed in this change. Decide
  whether to delete the commands (the `spawn_work` fn body stays — it backs
  `create_issue_and_spawn`) or keep them as a deliberate CLI-ish surface. M.
- **Non-atomic writes** for sessions / repo-settings / app-settings /
  identities (`std::fs::write`; `status.rs` already does temp+rename). A crash
  mid-write truncates the file, and for repo settings `load_validated` then
  *blocks spawn/teardown* until hand-repaired. A shared `write_json_atomic`
  also collapses four near-identical save fns. S.
- **Read-merge-write lost updates** between the window-size persisters and
  `app_settings_set` (and in `identities.rs`): interleaved saves can clobber
  each other's fields. A process-wide mutex around load/save. S.
- **`LogsView` polls forever while hidden** — the window hides instead of
  closing, and the 2 s `logs_read` poll (which reads the whole day's file
  server-side) keeps running all day. Gate on window visibility like the
  popover's `popoverOpen` gate; have `logs_read` read a bounded tail. S.
- **Overlay dialogs lack a11y semantics** (`HealthModal`, `HideSnoozeDialog`,
  the Start Work picker): no `role="dialog"`/`aria-modal`, no Escape handling,
  no focus trap; settings/issue inputs lack programmatic labels; the theme
  "radiogroup" has no arrow-key navigation. M.
- **Untested pure functions with real failure modes** in `spawn.rs`
  (`parse_issue_draft`, `slugify`/`default_short_title`/`trim_to_word`,
  `is_contained_relpath`, `shell_quote`, `merge_pill_state`/`pr_state`) and
  `status.rs` (`resolve_state`); `plugins/github.rs` has zero tests though
  `build_issue_node`/`issue_meta` are pure over JSON fixtures. ~40 lines of
  cheap tests. S.
- **`create_issue_and_spawn` discards the AI-drafted `short_title`** and lets
  `spawn_work` recompute a heuristic label, defeating the single-call design —
  relevant only if that path is kept (see dead-commands item above). S.

### Low (recommendations)

- Two `snippet` implementations with different caps/placeholders (`spawn.rs`
  240/`<empty>` vs `health.rs` 200/`<no output>`); `home()` re-derived in 7
  modules instead of a `pub fn` in `paths.rs`; `health.rs` re-implements the
  run-git-capture pattern instead of sharing `git()`; `session_pr` builds a
  `PrLink` inline five lines above the `pr_link_from` helper.
- `tools::init()`'s login-shell PATH probe has no timeout — a shell profile
  that prompts blocks startup with no tray icon.
- `worktree_in_use`'s prefix match lacks a path-separator check, so a sibling
  dir with an extending name counts as "in use" (fails conservative).
- Health's env-files check accepts absolute entries (`base.join(abs)` = the
  absolute path) that the spawn copy will reject via `is_contained_relpath` —
  run the same containment predicate in the check.
- Stringly-typed protocol fields in TS (`StatusRecord.state`, `PrLink.state`,
  `PrChecks.*`) — union types would catch typo'd comparisons (`HealthStatus`
  shows the pattern). `noUncheckedIndexedAccess` is off (latent hole; current
  index sites all guard). JsonForms `config` reads are untyped `any` flow
  despite declared `RepoFormConfig`/`AppFormConfig` interfaces.
- Error-type conventions: `credentials.rs` returns a typed `CredentialError`
  while everything else returns `Result<_, String>`; command naming mixes
  `noun_verb` and `verb_noun`. Worth one documented convention for new code.
- Frontend polish: Check Health button isn't disabled while a run is in flight
  (double-click interleaves two streams); per-session `openInEditor` rejections
  are silently dropped (repo-level button surfaces them); an unparseable custom
  snooze date makes Confirm a silent no-op; consecutive PR-check polls can land
  out of order (flickering dot only — auto-merge is correctly guarded).
- Branch-name scheme (`feature/`) and the 25-char slug algorithm are
  deliberately mirrored in TS (commented) — returning the derived strings in
  `SpawnPlan` would remove the duplication and show suffixed (`-2`) workspaces
  correctly in the preview.
- Min-window sizes are mirrored between `main.rs` constants and
  `tauri.conf.json` with no drift-guard test (currently in sync).
- The 500 kB bundle chunk is almost entirely JSON Forms — code-splitting the
  Settings window would shrink the popover's load, though for a local webview
  this is cosmetic.

## Verified sound (no change needed)

- **Logging**: all other registered commands (47 checked pre-fix, 51 of 51
  post-fix) carry `log_invoke!`/`log_invoke_debug!` with the documented
  info/debug split; `#[tracing::instrument]` `session=` spans present on the
  documented set (plus `session_merge_pr` now); GitHub calls flow through
  `GitHub::send` (all of them, post-fix); no credential value is ever logged
  (token only in the `Authorization` header; hook helper redacts URL userinfo).
- **Contract**: every command registered in `main.rs` has an exactly-named
  `api.ts` wrapper; all arg names correctly camelCased; return shapes,
  serde-tagged enums, and event names/payloads (`popover-shown`,
  `session-status`, `theme-changed`, `claude-activity`, `health-check`,
  `health-check-running`) match bidirectionally; status/PR/check state string
  literals are byte-identical across the boundary.
- **Schemas**: both hand-written schemas match their structs (drift-guard
  tests); defaults genuinely live only in the schemas; the JSON Forms layer
  strips `$schema`/`$id`/`default` as documented.
- **Backend correctness**: `unwrap`/`expect` sites are compile-time-safe or
  test-guarded; string slicing is ASCII-boundary-safe (`slugify`,
  `parse_short_label`, `redact_secrets`); the hook helper is failure-tolerant
  with validated workspace ids and atomic writes; teardown's editor-probe
  ordering and prefix-gated removal are coherent; ETag caching is correctly
  keyed; `clear_creating` can't clobber a hook-written status; no unsafe code.
- **Frontend**: all 9 Tauri event subscriptions clean up correctly (including
  unmount-before-resolve); all intervals/timeouts are cleared; the 6 s PR poll
  is gated on popover visibility with a latest-ref pattern and re-checks state
  before auto-merging; the cross-repo autosave guards (#65) hold; stable list
  keys throughout; zero explicit `any`; `strict` mode on.
- **Deliberate decisions correctly left alone**: launcher-not-host, no `gh`,
  ambient-env launches, schema-only defaults, lenient `repos_list` vs strict
  `load_validated`, last-write-wins status files, authoritative tool-path
  overrides, the health-check claude-envelope classification.

## Verification

After the applied fixes: `cargo build` clean, `cargo clippy --all-targets`
**0 warnings**, `cargo test` **42/42** (one new test), `pnpm build`
(tsc + vite) clean, `cargo audit` clean of the anyhow advisory.
