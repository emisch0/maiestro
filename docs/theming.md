# Worktree theming

How a spawned worktree gets its color and emoji, and how that one color reaches the popover row, the VS Code window, and the Claude session. The code lives in `backend/src/theming.rs` (`pick_theme`, `claude_color`) and `backend/src/editor.rs` (`write_vscode_files`).

A spawned worktree gets a deterministic color and emoji from `theming::pick_theme`, seeded by the workspace name and avoiding colors already claimed by tracked sessions. That one color is applied in three places so the visual thread survives from the dashboard to where the user actually works: the popover row, VS Code's title/status/activity bars (`editor::write_vscode_files`), and the Claude session's own UI.

`PALETTE` is deliberately sized and ordered to match Claude Code's eight session colors (`red, blue, green, yellow, purple, orange, pink, cyan`) one-for-one, so `theming::claude_color` is a **bijection**: no two live sessions collide on a session color while a distinct one goes unused. A unit test enforces this, so adding a ninth palette entry fails the build rather than silently doubling up. `claude_color` is keyed off each session's **stored** hex and also maps a retired palette color, so a session recorded under an older palette keeps the color it was given.

The color reaches the session as a **`/color <name>` initial prompt** appended to the launch command:

```
'/opt/homebrew/bin/claude' --remote-control --name '🎀 #127 — Add session color' '/color pink'
```

The binary is the **resolved** `claude` path (`tools::resolve_tool`), not a bare `claude` left to PATH. The task runs in VS Code's integrated terminal, whose PATH is whatever that VS Code process inherited — and a VS Code launched by a packaged mAIestro at login can carry the minimal Launch Services PATH, with no `claude` on it. So the same `tool_paths.claude` override that pins mAIestro's own drafting calls also decides which binary the *session* starts with; when nothing concrete resolves, `resolve_tool` yields the bare name, leaving it to PATH. Existing worktrees pick the resolved path up on their next reopen, via the `refresh_vscode_files` regeneration described below.

Not via a flag: Claude Code's `--agent-color` is only honored alongside `--agent-id`/`--agent-name`/`--team-name` (teammate sessions) and is **silently ignored** on its own. A leading-slash initial prompt is dispatched as a command, so this costs one line in the transcript and no model call.

Because the launch command lives in the worktree's generated `.vscode/tasks.json`, a worktree keeps whatever its original spawn baked in. So the **reuse** spawn path regenerates the `.vscode` files (`spawn::refresh_vscode_files`), mirroring what `reconcile_session_hooks` does for hooks: reopening picks up changes to what we generate, so a worktree spawned by an older build is brought up to date on its next reopen. The values come from the **session record**, not the caller's freshly picked theme — reopening must never re-theme a worktree. It is best-effort: no session record means no rewrite, and a write failure only warns. Note this overwrites `.vscode/settings.json` and `tasks.json` on every reopen, so hand edits to those generated files do not survive.

