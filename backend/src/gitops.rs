//! Thin `git` subprocess helpers shared across the spawn/PR/teardown paths.
//!
//! `git()` runs a git command in a directory and captures its trimmed stdout (or
//! a formatted error); `local_branch_exists()` is the common "does this branch
//! exist locally?" check. Both go through `tools::tokio_command("git")` so git
//! resolves under the app's minimal launch PATH (see `tools.rs`) *and* run
//! asynchronously — a blocking `std::process::Command` reached from an async Tauri
//! command would pin a tokio worker (issue #101). Extracted from `spawn.rs`
//! (issue #99) so hooks/editor/pr/spawn all share one implementation.

use std::path::Path;
use std::time::Duration;

/// Network git ops (`fetch`/`push`) can wedge on a black-holed SSH/HTTPS
/// connection. Bound them via [`git_net`] so a hung connection surfaces as an
/// error the UI can recover from (the "Merging…"/"Creating…" pill resolves)
/// instead of pinning a tokio worker forever. Local ops are unbounded (`git`) —
/// they can't stall on the network. Mirrors `claude`'s 90s / post-spawn's 600s
/// bounds (#101).
pub const GIT_NET_TIMEOUT: Duration = Duration::from_secs(120);

/// Run `git -C <dir> <args…>` asynchronously and return its trimmed stdout, or an
/// `Err` carrying the command and stderr on a non-zero exit. `timeout` bounds
/// network ops; local ops pass `None`. `kill_on_drop` ensures a timed-out or
/// cancelled git is reaped, not left mutating the worktree.
async fn git_run(dir: &Path, args: &[&str], timeout: Option<Duration>) -> Result<String, String> {
    let mut cmd = crate::tools::tokio_command("git");
    cmd.arg("-C").arg(dir).args(args).kill_on_drop(true);
    let out = match timeout {
        Some(d) => tokio::time::timeout(d, cmd.output())
            .await
            .map_err(|_| format!("git {} timed out after {}s", args.join(" "), d.as_secs()))?
            .map_err(|e| format!("failed to run git: {e}"))?,
        None => cmd.output().await.map_err(|e| format!("failed to run git: {e}"))?,
    };
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// A local (network-free) git op — unbounded, since it can't stall on a wedged
/// connection. Most call sites use this.
pub async fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    git_run(dir, args, None).await
}

/// A network git op (`fetch`/`push`), bounded by [`GIT_NET_TIMEOUT`].
pub async fn git_net(dir: &Path, args: &[&str]) -> Result<String, String> {
    git_run(dir, args, Some(GIT_NET_TIMEOUT)).await
}

/// True when a local branch `name` exists in the checkout at `cloned_repo`.
pub async fn local_branch_exists(cloned_repo: &Path, name: &str) -> bool {
    crate::tools::tokio_command("git")
        .arg("-C")
        .arg(cloned_repo)
        .args(["rev-parse", "--verify", "--quiet", &format!("refs/heads/{name}")])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}
