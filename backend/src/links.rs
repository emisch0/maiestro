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
