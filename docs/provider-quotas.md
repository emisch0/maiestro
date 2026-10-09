# Provider quotas

The popover shows a **quota strip** along its bottom edge, right-aligned: one chip per agent account behind the repos and work items currently on screen, with a vertical rule between chips. Each chip has a small meter per rate-limit window, e.g. Claude `5h 42% · 7d 18%`, Codex `30d 5%`, Copilot `premium 75%` or `plan 7%`. The chip's tooltip carries the plan, each window's reset time and how old the reading is. Hovering a chip shows it after about a second (`HOVER_DELAY_MS`); clicking shows it at once and keeps it open until a second click, a click elsewhere, Escape or the popover hiding. It is our own tooltip rather than a native `title`, since a webview can't open a native one on demand, and a single tooltip serves both, so the two never stack. It opens centered just above the cursor (where it was when the tooltip appeared), kept inside the window, and updates while open. A meter's color is a forecast rather than a fixed threshold (`quotaLevel` in `QuotaStrip.tsx`):

- Under 50 % used, it is green.
- At 100 % or more, it is red: the window is exhausted.
- Otherwise the usage so far is projected linearly to the reset: `used ÷ fraction of the window elapsed`. On pace for at most 100 % is green, up to 125 % amber, beyond that red. So 60 % used with one hour of a 5h window left (on pace for 75 %) is green, while 60 % halfway through (120 %) is amber.
- A window whose length or reset isn't known falls back to amber from 80 % and red from 95 %.

Each window carries its length (`window_secs`) for this: 5h and 7d for Claude, the reported window for Codex, and the calendar month before the reset date for Copilot. Past 50 %, the tooltip also says whether the window is on pace to last until the reset or, if not, when it runs out at the current pace. A window whose reset time has passed shows `—` rather than a stale percentage. With nothing to show, the strip isn't rendered.

The backend lives in `backend/src/quota.rs`; the strip in `frontend/src/components/QuotaStrip.tsx`.

## Rules

- **Per account, not per session.** Every launched session runs under the user's ambient agent login, so an agent appears once however many sessions use it. Copilot is read with each repo identity's own GitHub token, so it shows one chip per distinct GitHub login.
- **Visible repos and items.** The popover passes `provider_quotas_list(targets)` an agent + repo pair for each visible work item (its recorded agent) and for each visible repo (its effective agent: its own `agent`, else the global default from `app_agent_get`), following the same hidden/snoozed filtering as the list. So a repo shows its agent's chip even before it has any work items. An agent is read only when a target uses it, so a machine whose repos all resolve to Claude never runs `codex` and never calls GitHub for Copilot.
- **Read-only and soft-fail.** An agent with no source, or whose read fails, has no chip. Failures log at `warn`; nothing errors in the UI or blocks a spawn.
- **Antigravity is not read.** Its only source is an internal quota endpoint that is known to report wrong numbers, and `agy /usage` is interactive only.

## Sources

| Agent | Source | Freshness |
|---|---|---|
| Claude | The status-line JSON's `rate_limits.five_hour` / `seven_day` → `{ used_percentage, resets_at }` ([documented](https://code.claude.com/docs/en/statusline)). Present for Pro/Max after the session's first reply. | Live: every status-line redraw of any Claude session |
| Codex | The last `token_count` event's `rate_limits` in the newest rollout log under `$CODEX_HOME/sessions/YYYY/MM/DD/` (default `~/.codex`); else `codex app-server` → `account/rateLimits/read` | The log when written in the last 10 min; else the app-server, cached 60 s |
| Copilot | `GET api.github.com/copilot_internal/user` → `quota_snapshots.premium_interactions` / `chat` (`percent_remaining`, `has_quota`, `unlimited`) and `quota_reset_date_utc`. Undocumented. | Cached 60 s per identity |

### Claude: the chaining status line

Hook payloads don't carry the quota; only the status line's stdin does. So each Claude worktree's `.claude/settings.local.json` gets, next to our hooks:

```json
"statusLine": { "type": "command", "command": "'<bin>' statusline --workspace '<ws-id>'" }
```

`maiestro statusline` is a hidden subcommand dispatched in `main()` before Tauri starts, like `hook`. It:

1. Reads the status-line JSON from stdin. When `rate_limits` is present, it atomically writes `~/.maiestro/quota/claude.json`. A watcher on that directory emits a `provider-quota` event, and the popover updates the chip live.
2. Runs the status line the user would otherwise see: `statusLine` from the project's `.claude/settings.json`, else `$CLAUDE_CONFIG_DIR/settings.json` (default `~/.claude/settings.json`), Claude's precedence below `settings.local.json`. It runs through `sh -c` in the same directory with the same stdin, and its stdout is passed through. On Windows it runs through Git Bash (`tools::git_bash`: `CLAUDE_CODE_GIT_BASH_PATH`, else the `bash.exe` beside the resolved `git`, else `%ProgramFiles%\Git\bin\bash.exe`), the shell Claude Code runs status lines with, so a bash status line prints exactly what it does outside a worktree; `cmd /C` is used only when no Git Bash exists. The shell gets no console window, since the release `maiestro.exe` is a GUI-subsystem app. With no user status line it prints nothing, as before.
3. Never blocks or crashes the session: the chained command is killed after 5 s, and every failure path exits 0.

Installing, reconciling and removing follow the hooks (`hooks/claude.rs`), matched by the trailing `--workspace '<ws-id>'`:

- **Spawn** adds it, copying the user's `padding`, unless the file already has a `statusLine` that isn't ours. That one is the user's and is never replaced; that worktree then records no Claude quota.
- **Reconcile** (startup and reopen) re-points ours at the running binary. Where our hooks exist and no `statusLine` is set, it also **adds** ours, so worktrees spawned before this feature pick it up on the next launch. This is the one thing reconcile adds rather than re-points.
- **Switching away from Claude** removes it with the hooks.

We never modify `~/.claude/settings.json` and never read Claude Code's OAuth token. The undocumented `api.anthropic.com/api/oauth/usage` endpoint other tools use would need that token from Claude Code's Keychain item (a cross-app Keychain prompt), and refreshing it could invalidate Claude Code's own login.

### Codex

Every running Codex session already logs its rate limits, so reading the newest rollout log costs no process. Only its last 512 KB is read, from the newest seven day folders, and only the general `codex` limit counts (a per-model limit isn't the account's quota). When no log was written in the last 10 minutes, one `initialize` + `account/rateLimits/read` round trip goes to `codex app-server` (the helper `hooks::codex_app_server_request`, shared with the hook-trust check), bounded at 10 s and cached 60 s. If that fails, the newest log's reading is shown anyway, with its age in the tooltip.

### Copilot

`GitHub::copilot_user` calls `copilot_internal/user` through the `GitHub::send` choke point with the identity's own token from the Keychain, the same token mAIestro Code uses for everything else. It is the endpoint Copilot's own clients read usage from; a standard OAuth token with no extra scope works. Premium requests are shown as `premium` when the plan meters them. On Copilot Free, which has no premium allowance, the CLI's requests draw from the `chat` quota instead; it is shown as `plan`, the name Copilot CLI's own status line uses ("Plan: 14/200"). Unlimited quotas and IDE completions are skipped. The quota is the account's, so if the identity's GitHub account isn't the one Copilot CLI is logged in as, the chip shows the identity's account (named in the tooltip).

## Refresh

The popover fetches on mount, on `popover-shown`, whenever the set of visible agent/repo pairs changes (including pinning a repo's agent from its menu), and every 60 s while it is open. Claude readings also arrive live through `provider-quota`.

## Gaps

- API-key (pay-as-you-go) Claude and Codex users have no rate limits, so they get no chip.
- A Claude reading is only as fresh as the last status-line redraw of any Claude session; it ages in the tooltip, and a window past its reset shows `—`.
- Token, cost and context-window usage are out of scope, as are warnings near a limit and blocking spawns on quota.
