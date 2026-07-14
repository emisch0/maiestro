//! Resolve a tracked repo's operational context — its validated settings, the
//! GitHub client for its assigned identity, and its local checkout path.
//!
//! Every spawn/draft/PR command starts by loading the repo's settings, requiring
//! an assigned identity, and building a `GitHub` client from it; four of them also
//! check that the configured `cloned_repo_dir` is a real git checkout. That block
//! was copy-pasted ~9× in `spawn.rs` (the "No identity assigned…" string appeared
//! nine times). `repo_context` and `validated_cloned_repo` are the single home for
//! it (issue #99).
//!
//! Commands that *soft-fail* when no identity is assigned (returning `Ok(None)`
//! rather than an error — `session_pr`, `session_pr_checks`, `teardown`) keep
//! their own `if let Some(identity_id)` handling; these helpers are for the sites
//! that error.

use std::path::PathBuf;

use crate::paths::expand_tilde;
use crate::plugins::GitHub;
use crate::repo_settings::{self, RepoSettings};

/// Load `repo`'s settings, require an assigned identity, and build the GitHub
/// client for it. The returned settings are the same value the identity came
/// from, so callers can go on to read `env_files`, `prompts`, `worktree_prefix`,
/// etc. without reloading.
pub async fn repo_context(repo: &str) -> Result<(RepoSettings, GitHub), String> {
    let settings = repo_settings::repo_settings_get(repo.to_string())?;
    let identity_id = settings
        .identity_id
        .clone()
        .ok_or_else(|| "No identity assigned to this repo. Set one in Settings → Repo.".to_string())?;
    let gh = GitHub::for_identity(&identity_id).await?;
    Ok((settings, gh))
}

/// Resolve and validate a repo's local checkout: tilde-expand `cloned_repo_dir`
/// and confirm it holds a `.git`, erroring otherwise. The path the spawn/draft
/// paths run `git` in and copy env files from.
pub fn validated_cloned_repo(settings: &RepoSettings) -> Result<PathBuf, String> {
    let cloned_repo = expand_tilde(settings.cloned_repo_dir.as_deref().unwrap_or_default());
    if !cloned_repo.join(".git").exists() {
        return Err(format!("cloned repo dir is not a git repo: {}", cloned_repo.display()));
    }
    Ok(cloned_repo)
}
