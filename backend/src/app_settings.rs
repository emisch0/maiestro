//! Global, app-wide settings stored at `~/.maiestro/settings.json`.
//!
//! Sibling to `profiles.json` and the per-repo `repos/<…>.json` files, but holds
//! configuration that is neither a credential nor repo-scoped. Today that is the
//! popover and Settings window persisted sizes (issue #40), the UI theme (issue #12),
//! and per-tool CLI path overrides (issue #85); the file is intentionally
//! human-editable and dotfile-manageable. A missing or partial file is fine —
//! every field is optional and defaults to "not set".
//!
//! Like per-repo settings, the file format has a hand-written JSON Schema
//! (`backend/schemas/app-settings.schema.json`) that is the spec; the structs
//! below must conform to it (the `schema_matches_struct` test enforces this).
//! The `app_settings_schema` command returns it to the Settings window's JSON
//! Forms renderer.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Persisted size of a window, in logical pixels. Restored on launch before the
/// window is first shown, saved when the window hides (popover on blur, the
/// Settings window when it loses focus or is closed).
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

/// Explicit filesystem paths for the external CLIs mAIestro invokes directly.
/// Each field `None`/empty means "auto-resolve" (see `crate::tools`). Set one to
/// pin a specific binary — useful when the app, launched from `/Applications`
/// with a minimal `$PATH`, can't find a tool (issue #85).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolPaths {
    /// Path to the `claude` CLI (AI drafting). `None`/empty = auto-resolve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<String>,
    /// Path to `git` (worktree add, branch checks). `None`/empty = auto-resolve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    /// Path to the VS Code `code` CLI (open worktrees). `None`/empty = auto-resolve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppSettings {
    /// Saved popover size. `None` (absent) means "use the tauri.conf.json default".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowSize>,
    /// Saved Settings-window size. `None` (absent) means "use the tauri.conf.json default".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_window: Option<WindowSize>,
    /// Chosen UI theme. `None` (absent) means `System`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<Theme>,
    /// Per-tool CLI path overrides. `None` (absent) means all tools auto-resolve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_paths: Option<ToolPaths>,
}

/// The user's explicit path override for a directly-invoked tool
/// (`claude`/`git`/`code`), if set and non-empty. Read by `crate::tools`. An
/// unknown tool name or an empty/whitespace value yields `None` (auto-resolve).
pub fn tool_path_override(name: &str) -> Option<String> {
    let tp = load().tool_paths?;
    let v = match name {
        "claude" => tp.claude,
        "git" => tp.git,
        "code" => tp.code,
        _ => None,
    };
    v.filter(|s| !s.trim().is_empty())
}

// ── Schema (hand-written spec; the structs must conform to it) ──────────────

/// The canonical schema for `~/.maiestro/settings.json`. Hand-written and
/// checked in — *not* generated from the struct. The `schema_matches_struct`
/// test fails the build if they drift apart. Embedded so validation and the
/// `app_settings_schema` command need no file at runtime.
const SCHEMA_JSON: &str = include_str!("../schemas/app-settings.schema.json");

fn schema_value() -> serde_json::Value {
    serde_json::from_str(SCHEMA_JSON).expect("embedded app-settings schema is valid JSON")
}

/// Validate a settings JSON value against the embedded schema, naming the failing
/// field(s) on error.
fn validate_against_schema(value: &serde_json::Value) -> Result<(), String> {
    let schema = schema_value();
    let validator =
        jsonschema::validator_for(&schema).map_err(|e| format!("internal schema error: {e}"))?;
    let errors: Vec<String> = validator
        .iter_errors(value)
        .map(|e| {
            let at = e.instance_path().to_string();
            let at = if at.is_empty() { "/".to_string() } else { at };
            format!("at `{at}`: {e}")
        })
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
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

/// Read and validate the global settings. Unlike the lenient `load()` (which
/// defaults on any problem, for hot-path reads), this returns a loud error when
/// the file is present but invalid — so the Settings window can surface a banner
/// instead of a form full of defaults that would clobber the file on save.
/// Missing file → defaults.
pub fn load_validated() -> Result<AppSettings, String> {
    let path = settings_path();
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(AppSettings::default()),
        Err(e) => return Err(format!("Failed to read {}: {e}", path.display())),
    };
    let label = path.display().to_string();
    let value: serde_json::Value =
        serde_json::from_str(&data).map_err(|e| format!("{label} is not valid JSON: {e}"))?;
    validate_against_schema(&value).map_err(|msg| format!("{label} failed validation — {msg}"))?;
    serde_json::from_value(value).map_err(|e| format!("{label} does not match AppSettings: {e}"))
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

/// The hand-written JSON Schema for the global settings file, for the Settings
/// window's JSON Forms renderer.
#[tauri::command]
pub fn app_settings_schema() -> serde_json::Value {
    crate::log_invoke_debug!("app_settings_schema");
    schema_value()
}

/// Read the full, validated global settings for the Settings form. Fallible so a
/// corrupt file surfaces as a banner rather than silent defaults.
#[tauri::command]
pub fn app_settings_get() -> Result<AppSettings, String> {
    crate::log_invoke_debug!("app_settings_get");
    load_validated()
}

/// Persist the settings edited in the Settings form. The form owns only the
/// user-editable fields (`theme`, `tool_paths`); the machine-managed window sizes
/// are load-merged from disk so a concurrent size save from another window isn't
/// clobbered. Validates before writing, and broadcasts `theme-changed` when the
/// theme actually changed so every window re-applies live.
#[tauri::command]
pub fn app_settings_set(app: tauri::AppHandle, settings: AppSettings) -> Result<(), String> {
    crate::log_invoke!("app_settings_set", theme = ?settings.theme);
    // Defense in depth: never persist a value the schema would reject on reload.
    let value = serde_json::to_value(&settings).map_err(|e| e.to_string())?;
    validate_against_schema(&value).map_err(|msg| format!("Invalid settings — {msg}"))?;

    let mut current = load();
    let theme_changed = current.theme != settings.theme;
    current.theme = settings.theme;
    current.tool_paths = settings.tool_paths;
    // window / settings_window are deliberately kept from disk (machine-managed).
    save(&current).map_err(|e| e.to_string())?;

    if theme_changed {
        use tauri::Emitter;
        let _ = app.emit("theme-changed", current.theme.unwrap_or_default());
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;

    /// The embedded schema string must be valid JSON, since `schema_value`
    /// `.expect()`s it at runtime.
    #[test]
    fn schema_parses() {
        let v = schema_value();
        assert!(v.get("properties").is_some(), "schema must declare properties");
    }

    /// Drift guard: the hand-written schema and the Rust structs must describe the
    /// same set of top-level fields, and both a default and a fully-populated
    /// `AppSettings` must validate. Adding a field to one without the other fails
    /// here. The key-set check uses the fully-populated instance because every
    /// field is `skip_serializing_if = "Option::is_none"`, so `default` serializes
    /// to `{}`.
    #[test]
    fn schema_matches_struct() {
        let schema = schema_value();
        let schema_props: BTreeSet<String> = schema["properties"]
            .as_object()
            .expect("schema.properties is an object")
            .keys()
            .cloned()
            .collect();

        let populated = AppSettings {
            window: Some(WindowSize { width: 680.0, height: 460.0 }),
            settings_window: Some(WindowSize { width: 720.0, height: 520.0 }),
            theme: Some(Theme::Dark),
            tool_paths: Some(ToolPaths {
                claude: Some("/opt/homebrew/bin/claude".into()),
                git: Some("/opt/homebrew/bin/git".into()),
                code: Some("/usr/local/bin/code".into()),
            }),
        };
        let value = serde_json::to_value(&populated).unwrap();
        let struct_keys: BTreeSet<String> =
            value.as_object().unwrap().keys().cloned().collect();
        assert_eq!(
            schema_props, struct_keys,
            "schema properties and serialized AppSettings fields drifted apart"
        );
        validate_against_schema(&value)
            .unwrap_or_else(|e| panic!("populated AppSettings rejected by schema: {e}"));

        // A default (empty) instance validates too.
        let default = serde_json::to_value(AppSettings::default()).unwrap();
        validate_against_schema(&default)
            .unwrap_or_else(|e| panic!("default AppSettings rejected by schema: {e}"));
    }

    /// A partial file (only `theme`) validates — every field is optional.
    #[test]
    fn partial_file_validates() {
        validate_against_schema(&json!({ "theme": "light" })).expect("partial file should validate");
        validate_against_schema(&json!({ "tool_paths": { "claude": "/x/claude" } }))
            .expect("tool_paths-only file should validate");
    }

    /// A bad enum value or wrong type is rejected, naming the field.
    #[test]
    fn invalid_values_rejected() {
        assert!(validate_against_schema(&json!({ "theme": "chartreuse" })).is_err());
        assert!(validate_against_schema(&json!({ "tool_paths": { "git": 42 } })).is_err());
    }

    /// Unknown fields are tolerated (forward compat + hand-edited `$schema`).
    #[test]
    fn unknown_fields_tolerated() {
        validate_against_schema(&json!({
            "$schema": "./app-settings.schema.json",
            "theme": "system",
            "future_field": true
        }))
        .expect("unknown fields should be tolerated");
    }

    /// An empty/whitespace override reads as "auto-resolve" (None).
    #[test]
    fn blank_override_is_none() {
        let tp = ToolPaths { claude: Some("  ".into()), git: Some("".into()), code: None };
        // Exercise the same filter `tool_path_override` applies.
        assert!(tp.claude.as_deref().filter(|s| !s.trim().is_empty()).is_none());
        assert!(tp.git.as_deref().filter(|s| !s.trim().is_empty()).is_none());
    }
}
