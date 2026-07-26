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

/// The set of local branch short-names in the checkout at `cloned_repo`. Fetched
/// once so the spawn resolver can test candidate branch names *synchronously*,
/// keeping the naming/suffix logic a pure, unit-testable function instead of
/// interleaving an `.await` per candidate. Empty on any git error — the resolver
/// then only avoids on-disk worktree collisions, exactly the outcome a
/// non-existent branch (`local_branch_exists` == false) already produced.
pub async fn local_branches(cloned_repo: &Path) -> std::collections::HashSet<String> {
    let out = crate::tools::tokio_command("git")
        .arg("-C")
        .arg(cloned_repo)
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads"])
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        _ => std::collections::HashSet::new(),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// These drive the *real* `git` binary against throwaway repos in tempdirs — no
// mocks. Real git is fast and deterministic, and it's the only way to catch the
// class of bug mocks never will: dirty-tree detection, branch enumeration, and
// the worktree add/remove lifecycle the spawn/teardown paths depend on.
// `maiestro` is a bin-only crate (no lib target) so these can't live in
// `tests/`; they run in-crate where they can call the module directly.
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// A fresh git repo in a tempdir with one commit on branch `main`, isolated
    /// from the developer's global git config (identity + default branch pinned).
    async fn init_repo() -> (TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        git(&root, &["init", "-q", "-b", "main"]).await.expect("git init");
        git(&root, &["config", "user.email", "test@example.com"]).await.unwrap();
        git(&root, &["config", "user.name", "Test"]).await.unwrap();
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        git(&root, &["add", "."]).await.unwrap();
        git(&root, &["commit", "-q", "-m", "init"]).await.expect("git commit");
        (dir, root)
    }

    #[tokio::test]
    async fn git_returns_trimmed_stdout_and_errors_loudly() {
        let (_d, root) = init_repo().await;
        let branch = git(&root, &["rev-parse", "--abbrev-ref", "HEAD"]).await.unwrap();
        assert_eq!(branch, "main", "stdout is trimmed (no trailing newline)");

        // A bogus subcommand surfaces git's stderr in the Err.
        let err = git(&root, &["definitely-not-a-command"]).await.unwrap_err();
        assert!(err.contains("git definitely-not-a-command failed"), "got: {err}");
    }

    #[tokio::test]
    async fn local_branch_exists_and_local_branches_agree() {
        let (_d, root) = init_repo().await;
        git(&root, &["branch", "feature/x"]).await.unwrap();
        git(&root, &["branch", "feature/y"]).await.unwrap();

        assert!(local_branch_exists(&root, "main").await);
        assert!(local_branch_exists(&root, "feature/x").await);
        assert!(!local_branch_exists(&root, "feature/does-not-exist").await);

        let all = local_branches(&root).await;
        assert!(all.contains("main"));
        assert!(all.contains("feature/x"));
        assert!(all.contains("feature/y"));
        assert!(!all.contains("feature/z"));
    }

    #[tokio::test]
    async fn local_branches_empty_for_non_repo() {
        // A dir that isn't a git repo → empty set (git errors), matching the
        // "no branch exists" outcome the spawn resolver relies on.
        let dir = tempfile::tempdir().unwrap();
        assert!(local_branches(dir.path()).await.is_empty());
        assert!(!local_branch_exists(dir.path(), "main").await);
    }

    #[tokio::test]
    async fn worktree_add_dirty_check_and_remove() {
        let (_d, root) = init_repo().await;
        let wt = root.join("../wt-add-foo");

        // Add a worktree on a new branch off main — the spawn path's core op.
        git(
            &root,
            &["worktree", "add", &wt.to_string_lossy(), "-b", "feature/add-foo", "main"],
        )
        .await
        .expect("worktree add");
        assert!(wt.join("README.md").is_file(), "worktree checked out the tree");
        assert!(local_branch_exists(&root, "feature/add-foo").await);

        // Clean immediately after add: teardown's dirty check must read empty.
        let clean = git(&wt, &["status", "--porcelain"]).await.unwrap();
        assert_eq!(clean, "", "fresh worktree is not dirty");

        // Modify a tracked file → dirty.
        std::fs::write(wt.join("README.md"), "changed\n").unwrap();
        let dirty = git(&wt, &["status", "--porcelain"]).await.unwrap();
        assert!(!dirty.is_empty(), "modified worktree reads dirty");

        // Remove it (force: discards the change) — the teardown op.
        git(&root, &["worktree", "remove", "--force", &wt.to_string_lossy()])
            .await
            .expect("worktree remove");
        assert!(!wt.exists(), "worktree dir gone after remove");
    }

    #[tokio::test]
    async fn worktree_prune_tolerates_missing_dir() {
        // Teardown prunes a never-created worktree (issue #77). Prune on a repo
        // with no dangling worktrees is a clean no-op, not an error.
        let (_d, root) = init_repo().await;
        git(&root, &["worktree", "prune"]).await.expect("prune is a no-op");
    }
}
