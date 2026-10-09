//! Whether the brain icon is on the Windows taskbar or hidden in its `^`
//! overflow (#198), for the onboarding step that asks the user to drag it out.
//!
//! Windows 11 hides a new tray icon in the overflow and offers no API to pin
//! it (by design), but it records the user's choice per app under
//! `HKCU\Control Panel\NotifyIconSettings\<id>`: `ExecutablePath` names the
//! app and `IsPromoted` is `1` once the icon is on the taskbar (absent or `0`
//! while it's in the overflow). We only *read* that key, never write it.
//!
//! The answer is `None` when it can't be known: on macOS (the menu bar shows
//! every icon), on Windows 10 (which keeps this in an opaque blob instead), or
//! before Explorer has created our entry, which happens a moment after the
//! tray icon is first added.

use std::path::Path;

/// One `NotifyIconSettings` entry: its `ExecutablePath` and `IsPromoted`.
#[derive(Debug, Clone, PartialEq)]
pub struct IconEntry {
    pub exe: String,
    pub promoted: Option<u32>,
}

/// Whether `exe`'s icon is on the taskbar, from the registry entries. Paths
/// compare case-insensitively (Windows paths do). Several entries can name the
/// same exe (one per icon UID); any promoted one counts. `None` when no entry
/// names `exe`.
pub fn promoted_for(entries: &[IconEntry], exe: &Path) -> Option<bool> {
    let exe = exe.to_string_lossy();
    let mine: Vec<_> = entries.iter().filter(|e| e.exe.eq_ignore_ascii_case(&exe)).collect();
    if mine.is_empty() {
        return None;
    }
    Some(mine.iter().any(|e| e.promoted == Some(1)))
}

/// Whether the running app's tray icon is on the taskbar: `Some(true)` on it,
/// `Some(false)` hidden in the overflow, `None` unknown (see the module docs).
/// The onboarding window polls this, so it logs at `debug`.
#[tauri::command]
pub fn tray_icon_promoted() -> Option<bool> {
    crate::log_invoke_debug!("tray_icon_promoted");
    let exe = std::env::current_exe().ok()?;
    promoted_for(&registry::entries()?, &exe)
}

#[cfg(target_os = "windows")]
mod registry {
    use super::IconEntry;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ, RRF_RT_REG_DWORD,
        RRF_RT_REG_SZ,
    };

    const KEY: &str = r"Control Panel\NotifyIconSettings";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Every entry under `NotifyIconSettings`, or `None` when the key doesn't
    /// exist (Windows 10).
    pub fn entries() -> Option<Vec<IconEntry>> {
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: plain registry reads into buffers we own; the key opened here
        // is closed before returning.
        unsafe {
            if RegOpenKeyExW(HKEY_CURRENT_USER, wide(KEY).as_ptr(), 0, KEY_READ, &mut key) != ERROR_SUCCESS {
                return None;
            }
            let mut out = Vec::new();
            for index in 0.. {
                let mut name = [0u16; 256];
                let mut len = name.len() as u32;
                let rc = RegEnumKeyExW(
                    key,
                    index,
                    name.as_mut_ptr(),
                    &mut len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
                if rc != ERROR_SUCCESS {
                    break;
                }
                let sub = String::from_utf16_lossy(&name[..len as usize]);
                if let Some(exe) = string_value(key, &sub, "ExecutablePath") {
                    out.push(IconEntry { exe, promoted: dword_value(key, &sub, "IsPromoted") });
                }
            }
            RegCloseKey(key);
            Some(out)
        }
    }

    unsafe fn string_value(key: HKEY, sub: &str, value: &str) -> Option<String> {
        let mut buf = vec![0u16; 1024];
        let mut bytes = (buf.len() * 2) as u32;
        let rc = RegGetValueW(
            key,
            wide(sub).as_ptr(),
            wide(value).as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut bytes,
        );
        if rc != ERROR_SUCCESS {
            return None;
        }
        // `bytes` includes the terminating NUL.
        let len = (bytes as usize / 2).saturating_sub(1);
        Some(String::from_utf16_lossy(&buf[..len]))
    }

    unsafe fn dword_value(key: HKEY, sub: &str, value: &str) -> Option<u32> {
        let mut data = 0u32;
        let mut bytes = std::mem::size_of::<u32>() as u32;
        let rc = RegGetValueW(
            key,
            wide(sub).as_ptr(),
            wide(value).as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut data as *mut u32).cast(),
            &mut bytes,
        );
        (rc == ERROR_SUCCESS).then_some(data)
    }
}

#[cfg(not(target_os = "windows"))]
mod registry {
    use super::IconEntry;

    /// No overflow to hide in: the menu bar shows every icon.
    pub fn entries() -> Option<Vec<IconEntry>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(exe: &str, promoted: Option<u32>) -> IconEntry {
        IconEntry { exe: exe.to_string(), promoted }
    }

    const EXE: &str = r"C:\Users\me\AppData\Local\mAIestro Code\maiestro.exe";

    #[test]
    fn promoted_entry_is_on_the_taskbar() {
        let entries = [entry(r"C:\other\app.exe", None), entry(EXE, Some(1))];
        assert_eq!(promoted_for(&entries, Path::new(EXE)), Some(true));
    }

    #[test]
    fn missing_or_zero_is_promoted_means_hidden() {
        assert_eq!(promoted_for(&[entry(EXE, None)], Path::new(EXE)), Some(false));
        assert_eq!(promoted_for(&[entry(EXE, Some(0))], Path::new(EXE)), Some(false));
    }

    #[test]
    fn no_entry_for_this_exe_is_unknown() {
        // e.g. another worktree's dev build is promoted, but not this one.
        let entries = [entry(r"C:\src\other\maiestro.exe", Some(1))];
        assert_eq!(promoted_for(&entries, Path::new(EXE)), None);
    }

    #[test]
    fn paths_match_case_insensitively_and_any_uid_counts() {
        let entries = [entry(&EXE.to_uppercase(), Some(0)), entry(&EXE.to_lowercase(), Some(1))];
        assert_eq!(promoted_for(&entries, Path::new(EXE)), Some(true));
    }
}
