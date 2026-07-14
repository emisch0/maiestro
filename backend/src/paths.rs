//! Small filesystem-path helpers shared across the backend.
//!
//! `expand_tilde` was duplicated in `tools.rs` and `spawn.rs`; it lives here now
//! so the path-validation commands in `links.rs` (issue #88) and both of those
//! callers share one implementation.

use std::path::{Component, Path, PathBuf};

/// The user's home directory (`$HOME`, or an empty path if unset). Re-derived in
/// several modules before this was promoted here; they now call this one copy.
pub fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

/// A `~/.maiestro/<rel>` path — the app's config/state root under the home dir.
pub fn maiestro_dir(rel: &str) -> PathBuf {
    home().join(".maiestro").join(rel)
}

/// Write `data` to `path` atomically: write a sibling temp file, then rename it
/// over the target. A crash mid-write leaves the original intact rather than a
/// truncated file — important for the `~/.maiestro/` JSON configs, where a
/// truncated `repos/*.json` would make `load_validated` fail loudly and block
/// spawn/teardown until hand-repaired (#101). `status.rs` already did this for its
/// status records; this is the shared version for the sessions/repo/app/identity
/// writers. Rename is atomic only within a filesystem, so the temp lives in the
/// target's own directory (which the caller must ensure exists).
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent directory")
    })?;
    // Temp name derived from the target file name so concurrent writes to
    // *different* files never collide; same-file writes are serialized by the
    // caller's process-wide lock.
    let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("tmp");
    let tmp = dir.join(format!(".{file_name}.tmp"));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

/// Expand a leading `~` in a user-configured path to `$HOME`. Other paths pass
/// through unchanged.
pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest)
    } else if p == "~" {
        home()
    } else {
        PathBuf::from(p)
    }
}

/// Whether `rel` is a safe *relative* path to join onto a base dir: non-empty,
/// not absolute, and with no `..` component — so `base.join(rel)` cannot escape
/// the base. Shared by the spawn env-file copy and the repo-health env-file check
/// so both apply the same containment rule. A user-authored `env_files` entry is
/// normally a bare name like `.env.local`, but an absolute (`/Users/me/.ssh/…`) or
/// `..`-laden entry would otherwise read/write outside the intended directory.
pub fn is_contained_relpath(rel: &str) -> bool {
    !rel.is_empty()
        && Path::new(rel)
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_expands_leading_home() {
        let h = home();
        assert_eq!(expand_tilde("~/bin/x"), h.join("bin/x"));
        assert_eq!(expand_tilde("~"), h);
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
    }

    #[test]
    fn contained_relpath_accepts_plain_relatives() {
        assert!(is_contained_relpath(".env"));
        assert!(is_contained_relpath(".env.local"));
        assert!(is_contained_relpath("frontend/.env"));
        assert!(is_contained_relpath("./config/.env"));
    }

    #[test]
    fn contained_relpath_rejects_escapes() {
        assert!(!is_contained_relpath(""));
        assert!(!is_contained_relpath("/etc/passwd"));
        assert!(!is_contained_relpath("../secret"));
        assert!(!is_contained_relpath("a/../../b"));
    }

    #[test]
    fn write_atomic_creates_and_overwrites() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        // Unique dir per run + call, no external crates. pid + counter avoids
        // collisions between parallel test binaries.
        let dir = std::env::temp_dir().join(format!(
            "maiestro-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("data.json");

        write_atomic(&path, b"first").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");

        // Overwriting replaces the content and leaves no temp file behind.
        write_atomic(&path, b"second").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        assert!(!dir.join(".data.json.tmp").exists(), "temp file should be renamed away");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_atomic_rejects_parentless_path() {
        assert!(write_atomic(Path::new("/"), b"x").is_err());
    }
}
