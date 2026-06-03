//! Global, app-wide settings stored at `~/.maiestro/settings.json`.
//!
//! Sibling to `profiles.json` and the per-repo `repos/<…>.json` files, but holds
//! configuration that is neither a credential nor repo-scoped. Today that is just
//! the popover window's persisted size (issue #40); the file is intentionally
//! human-editable and dotfile-manageable. A missing or partial file is fine —
//! every field is optional and defaults to "not set".

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Persisted size of the menu-bar popover, in logical pixels. Restored on launch
/// before the popover is first shown, saved when the popover hides on blur.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WindowSize {
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppSettings {
    /// Saved popover size. `None` (absent) means "use the tauri.conf.json default".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowSize>,
}

// ── Storage ─────────────────────────────────────────────────────────────────

fn settings_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".maiestro/settings.json")
}

/// Read the global settings, returning defaults if the file is absent or unreadable.
pub fn load() -> AppSettings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|data| serde_json::from_str(&data).ok())
        .unwrap_or_default()
}

/// Write the global settings, creating `~/.maiestro/` if needed.
pub fn save(settings: &AppSettings) -> std::io::Result<()> {
    let path = settings_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let data = serde_json::to_string_pretty(settings).unwrap();
    std::fs::write(path, data)
}
