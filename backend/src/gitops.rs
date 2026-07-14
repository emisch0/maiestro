//! Thin `git` subprocess helpers shared across the spawn/PR/teardown paths.
//!
//! `git()` runs a git command in a directory and captures its trimmed stdout (or
//! a formatted error); `local_branch_exists()` is the common "does this branch
//! exist locally?" check. Both go through `tools::command("git")` so git resolves
//! under the app's minimal launch PATH (see `tools.rs`). Extracted from `spawn.rs`
//! (issue #99) so hooks/editor/pr/spawn all share one implementation.

use std::path::Path;

/// Run `git -C <dir> <args…>` and return its trimmed stdout, or an `Err` carrying
/// the command and stderr on a non-zero exit.
pub fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = crate::tools::command("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// True when a local branch `name` exists in the checkout at `cloned_repo`.
pub fn local_branch_exists(cloned_repo: &Path, name: &str) -> bool {
    crate::tools::command("git")
        .arg("-C")
        .arg(cloned_repo)
        .args(["rev-parse", "--verify", "--quiet", &format!("refs/heads/{name}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
