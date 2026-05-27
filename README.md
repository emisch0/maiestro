# mAIestro

A menu-bar launcher for per-issue development workspaces. It creates and tears
down git worktrees (one per GitHub issue) behind a small always-on menu-bar UI.

Built with [Tauri v2](https://tauri.app) — a Rust backend with a React + Vite +
TypeScript frontend. macOS is the primary target; Windows/Linux are a goal.

> **Status:** Phase 0 — app shell only. A menu-bar (tray) icon that toggles a
> window. Worktree and GitHub logic come later. See issue #1.

## Project layout

```
maiestro/
  backend/          # Rust / Tauri: tray icon, window toggle, activation policy
    src/main.rs
    tauri.conf.json
    icons/          # app + tray icons (wizard-hat placeholder)
  frontend/         # React + Vite frontend (the window content)
  vite.config.ts    # Vite config (root = frontend/)
  package.json      # JS deps + scripts (dev/build/tauri)
```

## Prerequisites

Install these once on your machine. Versions in parentheses are what this was
developed against.

| Tool | Why | Install |
| --- | --- | --- |
| **Homebrew** | macOS package manager used to install Node/pnpm | `/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"` |
| **Xcode Command Line Tools** | C toolchain + macOS SDK that Rust/Tauri link against | `xcode-select --install` |
| **Rust** (1.95) | Compiles the Tauri backend | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| **Node.js** (25) | Runs Vite and the Tauri CLI | `brew install node` |
| **pnpm** (10) | Package manager for the frontend | `brew install pnpm` |

After installing Rust, restart your shell (or `source "$HOME/.cargo/env"`) so
`cargo` is on your `PATH`.

> On Windows/Linux the Node/pnpm/Rust steps are the same; the system
> prerequisites differ (Tauri's [prerequisites guide](https://tauri.app/start/prerequisites/)
> lists the WebView2 / `webkit2gtk` packages you'll need instead of Xcode).

## Setup

```bash
pnpm install
```

This installs the frontend dependencies and the Tauri CLI. The first run also
fetches Rust crates (handled automatically on first build).

## Develop

```bash
pnpm tauri dev
```

This starts the Vite dev server and launches the app. A wizard-hat icon appears
in the menu bar — click it to toggle the window. The first build compiles the
full Rust dependency tree and takes a few minutes; later builds are incremental.

## Build a release bundle

```bash
pnpm tauri build
```

Produces a `.app` (and `.dmg`) under `backend/target/release/bundle/`.

> Release bundles are unsigned for now, so macOS Gatekeeper will warn on first
> open (right-click → Open to bypass). Signing/notarization is out of scope for
> Phase 0.

## Regenerating icons

The icon set in `backend/icons/` is generated from a single source PNG:

```bash
pnpm tauri icon path/to/source.png -o backend/icons
```
