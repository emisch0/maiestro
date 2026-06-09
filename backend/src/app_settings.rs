//! Global, app-wide settings stored at `~/.maiestro/settings.json`.
//!
//! Sibling to `profiles.json` and the per-repo `repos/<…>.json` files, but holds
//! configuration that is neither a credential nor repo-scoped. Today that is the
//! popover window's persisted size (issue #40) and the UI theme (issue #12); the
//! file is intentionally human-editable and dotfile-manageable. A missing or
//! partial file is fine — every field is optional and defaults to "not set".

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Persisted size of the menu-bar popover, in logical pixels. Restored on launch
/// before the popover is first shown, saved when the popover hides on blur.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WindowSize {
    pub width: f64,
    pub height: f64,
}

/// The UI appearance the user picked. `System` follows the macOS dark/light
/// setting (resolved in the frontend via `prefers-color-scheme`). Absent in the
/// settings file means `System`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    Dark,
    #[default]
    System,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppSettings {
    /// Saved popover size. `None` (absent) means "use the tauri.conf.json default".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowSize>,
    /// Chosen UI theme. `None` (absent) means `System`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<Theme>,
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

// ── Commands ──────────────────────────────────────────────────────────────

/// Read the persisted UI theme (`System` when unset).
#[tauri::command]
pub fn app_settings_get_theme() -> Theme {
    crate::log_invoke_debug!("app_settings_get_theme");
    load().theme.unwrap_or_default()
}

/// Persist the chosen UI theme and broadcast `theme-changed` so every open
/// window (popover, settings, logs) re-applies it live. Load-merges so the
/// popover size and any other fields are preserved.
#[tauri::command]
pub fn app_settings_set_theme(app: tauri::AppHandle, theme: Theme) -> Result<(), String> {
    crate::log_invoke!("app_settings_set_theme", theme = ?theme);
    let mut settings = load();
    settings.theme = Some(theme);
    save(&settings).map_err(|e| e.to_string())?;
    use tauri::Emitter;
    let _ = app.emit("theme-changed", theme);
    Ok(())
}
