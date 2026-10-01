use crate::tools::{os_open, os_reveal, spawn_reaped};

/// Opens a URL in the user's default browser via the OS (`open` on macOS, the
/// shell's URL handler on Windows — see `tools::os_open`). Consistent with the
/// rest of the app, which hands off to the OS rather than constructing its own
/// environment or bundling a webview navigation.
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    crate::log_invoke!("open_url", url = %url);
    // Only hand http(s) URLs to the OS so a malformed value can't invoke
    // `open` with an unexpected scheme or local path.
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(format!("refusing to open non-http url: {url}"));
    }
    spawn_reaped(&mut os_open(url.as_ref())).map_err(|e| format!("failed to open {url}: {e}"))?;
    Ok(())
}

/// Opens a local path in Finder / Explorer (a directory opens that folder).
#[tauri::command]
pub fn open_path(path: String) -> Result<(), String> {
    crate::log_invoke!("open_path", path = %path);
    let p = crate::paths::expand_tilde(&path);
    if !p.exists() {
        return Err(format!("path does not exist: {path}"));
    }
    spawn_reaped(&mut os_open(p.as_os_str())).map_err(|e| format!("failed to open {path}: {e}"))?;
    Ok(())
}

/// Whether a user-configured path exists on disk. Tilde-expanded (`~` → home)
/// to match how the backend resolves these paths at spawn/resolve time. Empty or
/// whitespace-only input is treated as "not a path" and returns `false`. Backs
/// the soft path validation in the Settings window (issue #88); read-only and
/// polled by the form, hence `debug`.
#[tauri::command]
pub fn path_exists(path: String) -> bool {
    crate::log_invoke_debug!("path_exists", path = %path);
    if path.trim().is_empty() {
        return false;
    }
    crate::paths::expand_tilde(&path).exists()
}

/// Reveals a local path in Finder / Explorer, selecting it in its parent folder
/// (`tools::os_reveal`), mirroring `logs_reveal`. Distinct from `open_path`, which
/// opens the target itself: revealing an env *file* must select it rather than
/// launch it in its default app. Tilde-expanded; errors if the path is missing.
#[tauri::command]
pub fn reveal_path(path: String) -> Result<(), String> {
    crate::log_invoke!("reveal_path", path = %path);
    let p = crate::paths::expand_tilde(&path);
    if !p.exists() {
        return Err(format!("path does not exist: {path}"));
    }
    spawn_reaped(&mut os_reveal(&p)).map_err(|e| format!("failed to reveal {path}: {e}"))?;
    Ok(())
}
