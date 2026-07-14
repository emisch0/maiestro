use std::process::Command;

/// Opens a URL in the user's default browser via macOS Launch Services.
/// Consistent with the rest of the app, which hands off to `open` rather than
/// constructing its own environment or bundling a webview navigation.
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    crate::log_invoke!("open_url", url = %url);
    // Only hand http(s) URLs to the OS so a malformed value can't invoke
    // `open` with an unexpected scheme or local path.
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(format!("refusing to open non-http url: {url}"));
    }
    Command::new("open")
        .arg(&url)
        .spawn()
        .map_err(|e| format!("failed to open {url}: {e}"))?;
    Ok(())
}

/// Opens a local path in Finder (a directory opens that folder).
#[tauri::command]
pub fn open_path(path: String) -> Result<(), String> {
    crate::log_invoke!("open_path", path = %path);
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err(format!("path does not exist: {path}"));
    }
    Command::new("open")
        .arg(p)
        .spawn()
        .map_err(|e| format!("failed to open {path}: {e}"))?;
    Ok(())
}

/// Whether a user-configured path exists on disk. Tilde-expanded (`~` → `$HOME`)
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

/// Reveals a local path in Finder, selecting it in its parent folder (`open -R`),
/// mirroring `logs_reveal`. Distinct from `open_path`, which opens the target
/// itself: `-R` is required so revealing an env *file* selects it rather than
/// launching it in its default app. Tilde-expanded; errors if the path is missing.
#[tauri::command]
pub fn reveal_path(path: String) -> Result<(), String> {
    crate::log_invoke!("reveal_path", path = %path);
    let p = crate::paths::expand_tilde(&path);
    if !p.exists() {
        return Err(format!("path does not exist: {path}"));
    }
    Command::new("open")
        .arg("-R")
        .arg(&p)
        .spawn()
        .map_err(|e| format!("failed to reveal {path}: {e}"))?;
    Ok(())
}
