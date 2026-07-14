//! Small filesystem-path helpers shared across the backend.
//!
//! `expand_tilde` was duplicated in `tools.rs` and `spawn.rs`; it lives here now
//! so the path-validation commands in `links.rs` (issue #88) and both of those
//! callers share one implementation.

use std::path::PathBuf;

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
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
}
