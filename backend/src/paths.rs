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
}
