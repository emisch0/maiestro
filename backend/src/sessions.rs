//! Spawned-session registry: one JSON file per session under
//! `~/.maiestro/sessions/<id>.json`. Records the details of a launched
//! workspace (worktree path, branch, theming, originating issue) so mAIestro
//! can reason about what's in flight — e.g. which title-bar colors are taken —
//! without scraping each worktree's `.vscode/settings.json`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Unique, human-readable id (the workspace name, e.g. "488-wider-window").
    pub id: String,
    /// "owner/name" of the originating repo.
    pub repo: String,
    pub issue_number: u64,
    pub issue_url: String,
    pub branch: String,
    /// Absolute path to the spawned worktree.
    pub work_dir: String,
    /// Absolute path to the source checkout the worktree was created from.
    pub checkout_dir: String,
    /// Human-facing session name, including the leading emoji.
    pub session_title: String,
    /// Title-bar background color (hex).
    pub color: String,
    pub emoji: String,
}

fn sessions_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".maiestro/sessions")
}

fn session_path(id: &str) -> PathBuf {
    sessions_dir().join(format!("{id}.json"))
}

pub fn save(session: &Session) -> std::io::Result<()> {
    let dir = sessions_dir();
    std::fs::create_dir_all(&dir)?;
    let data = serde_json::to_string_pretty(session).unwrap();
    std::fs::write(session_path(&session.id), data)
}

pub fn load_all() -> Vec<Session> {
    let Ok(entries) = std::fs::read_dir(sessions_dir()) else { return Vec::new(); };
    entries
        .flatten()
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "json"))
        .filter_map(|e| {
            let data = std::fs::read_to_string(e.path()).ok()?;
            serde_json::from_str(&data).ok()
        })
        .collect()
}

/// Title-bar colors already claimed by tracked sessions.
pub fn used_colors() -> Vec<String> {
    load_all().into_iter().map(|s| s.color).collect()
}

#[tauri::command]
pub fn sessions_list() -> Vec<Session> {
    load_all()
}
