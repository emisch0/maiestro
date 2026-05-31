use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoSettings {
    /// Canonical "owner/name" string. Stored inside the file so repos_list can
    /// reconstruct the list without parsing the filename (which uses "-" and is ambiguous).
    #[serde(default)]
    pub repo: String,
    /// Identity used for credentials and agent spawning for this repo.
    pub identity_id: Option<String>,
    /// Absolute path to the local checkout directory.
    pub checkout_dir: Option<String>,
    /// Env files relative to checkout_dir to source when launching user-facing tools.
    pub env_files: Vec<String>,
}

impl RepoSettings {
    fn default_for(repo: &str) -> Self {
        let name = repo.split('/').next_back().unwrap_or(repo);
        let home = std::env::var("HOME").unwrap_or_default();
        Self {
            repo: repo.to_owned(),
            identity_id: None,
            checkout_dir: Some(format!("{home}/src/{name}")),
            env_files: Vec::new(),
        }
    }
}

// ── Storage ───────────────────────────────────────────────────────────────────

fn repos_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".maiestro/repos")
}

fn settings_path(repo: &str) -> PathBuf {
    let filename = repo.replace('/', "-") + ".json";
    repos_dir().join(filename)
}

fn load(repo: &str) -> Option<RepoSettings> {
    let data = std::fs::read_to_string(settings_path(repo)).ok()?;
    serde_json::from_str(&data).ok()
}

fn save(repo: &str, settings: &RepoSettings) -> std::io::Result<()> {
    let dir = repos_dir();
    std::fs::create_dir_all(&dir)?;
    let data = serde_json::to_string_pretty(settings).unwrap();
    std::fs::write(settings_path(repo), data)
}

// ── Env file scanner ──────────────────────────────────────────────────────────

const SKIP_DIRS: &[&str] = &[
    ".git", "node_modules", "target", "vendor", ".cache",
    "dist", ".next", ".nuxt", "__pycache__", ".venv", "venv",
];

fn walk_env_files(dir: &Path, depth: u8, results: &mut Vec<String>) {
    if depth == 0 { return; }
    let Ok(entries) = std::fs::read_dir(dir) else { return; };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if path.is_dir() {
            if !SKIP_DIRS.contains(&name_str.as_ref()) {
                walk_env_files(&path, depth - 1, results);
            }
        } else if name_str.as_ref() == ".env" {
            if let Some(s) = path.to_str() {
                results.push(s.to_owned());
            }
        }
    }
}

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
pub fn repos_list() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(repos_dir()) else { return Vec::new(); };
    let mut repos: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "json"))
        .filter_map(|e| {
            let data = std::fs::read_to_string(e.path()).ok()?;
            let s: RepoSettings = serde_json::from_str(&data).ok()?;
            if s.repo.is_empty() { None } else { Some(s.repo) }
        })
        .collect();
    repos.sort();
    repos
}

#[tauri::command]
pub fn repo_settings_get(repo: String) -> RepoSettings {
    load(&repo).unwrap_or_else(|| RepoSettings::default_for(&repo))
}

#[tauri::command]
pub fn repo_settings_set(repo: String, mut settings: RepoSettings) -> Result<(), String> {
    settings.repo = repo.clone();
    save(&repo, &settings).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn repo_scan_env_files(checkout_dir: String) -> Vec<String> {
    let base = Path::new(&checkout_dir);
    let mut abs = Vec::new();
    walk_env_files(base, 4, &mut abs);
    abs.iter()
        .filter_map(|p| Path::new(p).strip_prefix(base).ok())
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}
