use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

/// Hide/snooze state for a repo or a work item. The presence of this value means
/// "hidden"; its absence means "visible".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HideState {
    /// Unix-epoch millis until which the item stays hidden. `None` = hidden
    /// indefinitely. Expiry (now past `snooze_until`) is resolved on the frontend
    /// at render time; the backend stores the timestamp verbatim.
    #[serde(default)]
    pub snooze_until: Option<i64>,
}

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
    /// Prefix for worktree locations. The full worktree path is
    /// `<worktree_prefix><workspace>/<repo>` (a string concatenation — the
    /// trailing segment is part of the directory name, not a path component).
    /// `None` falls back to `~/src/work-` at the use site, preserving the
    /// original hardcoded behavior. Tilde-expanded via `expand_tilde` in spawn.
    #[serde(default)]
    pub worktree_prefix: Option<String>,
    /// Env files relative to checkout_dir to source when launching user-facing tools.
    pub env_files: Vec<String>,
    /// Repo-level hide/snooze state. `None` = visible. A hidden repo hides its
    /// work items too. Per-work-item state lives on the session record, not here.
    #[serde(default)]
    pub hidden: Option<HideState>,
}

impl RepoSettings {
    fn default_for(repo: &str) -> Self {
        let name = repo.split('/').next_back().unwrap_or(repo);
        let home = std::env::var("HOME").unwrap_or_default();
        Self {
            repo: repo.to_owned(),
            identity_id: None,
            checkout_dir: Some(format!("{home}/src/{name}")),
            worktree_prefix: None,
            env_files: Vec::new(),
            hidden: None,
        }
    }
}

// ── Schema (hand-written spec; the struct must conform to it) ───────────────────

/// The canonical schema for the on-disk file format. Hand-written and checked in
/// at `backend/schemas/repo-settings.schema.json` — *not* generated from the
/// struct. `RepoSettings`/`HideState` are obligated to match it; the
/// `schema_matches_struct` test fails the build if they drift apart. Embedded so
/// validation and the `repo_settings_schema` command need no file at runtime.
const SCHEMA_JSON: &str = include_str!("../schemas/repo-settings.schema.json");

/// Parse the embedded schema. Infallible in practice — the `schema_parses` test
/// guarantees the embedded string is valid JSON, so a panic here is a build bug.
fn schema_value() -> serde_json::Value {
    serde_json::from_str(SCHEMA_JSON).expect("embedded repo-settings schema is valid JSON")
}

/// Validate a settings JSON value against the embedded schema. Returns a message
/// naming the failing field(s) on error.
fn validate_against_schema(value: &serde_json::Value) -> Result<(), String> {
    let schema = schema_value();
    let validator = jsonschema::validator_for(&schema)
        .map_err(|e| format!("internal schema error: {e}"))?;
    let errors: Vec<String> = validator
        .iter_errors(value)
        .map(|e| {
            let at = e.instance_path().to_string();
            let at = if at.is_empty() { "/".to_string() } else { at };
            format!("at `{at}`: {e}")
        })
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// Write the embedded schema to `~/.maiestro/schemas/repo-settings.schema.json`
/// at startup so hand-editors can point a `$schema` key at it for autocomplete.
/// Best-effort: a failure is logged, not fatal.
pub fn write_schema_file() {
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = PathBuf::from(home).join(".maiestro/schemas");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(error = %e, "failed to create ~/.maiestro/schemas");
        return;
    }
    let path = dir.join("repo-settings.schema.json");
    if let Err(e) = std::fs::write(&path, SCHEMA_JSON) {
        tracing::warn!(error = %e, path = %path.display(), "failed to write repo-settings schema");
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

/// Load and validate a repo's settings file. Three outcomes:
/// - missing file → defaults (unchanged behavior);
/// - present but invalid (bad JSON or schema violation) → a loud error naming the
///   file and the failing field, so callers fail instead of silently clobbering a
///   hand-edited file with defaults;
/// - valid → the parsed settings.
fn load_validated(repo: &str) -> Result<RepoSettings, String> {
    let path = settings_path(repo);
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RepoSettings::default_for(repo));
        }
        Err(e) => return Err(format!("Failed to read {}: {e}", path.display())),
    };
    parse_and_validate(&data, &path.display().to_string())
}

/// Parse JSON, validate it against the schema, then deserialize. `label` names
/// the source (a file path) in error messages. Split out from `load_validated`
/// so the validation behavior is unit-testable without touching the filesystem.
fn parse_and_validate(data: &str, label: &str) -> Result<RepoSettings, String> {
    let value: serde_json::Value = serde_json::from_str(data)
        .map_err(|e| format!("{label} is not valid JSON: {e}"))?;
    validate_against_schema(&value).map_err(|msg| format!("{label} failed validation — {msg}"))?;
    serde_json::from_value(value).map_err(|e| format!("{label} does not match RepoSettings: {e}"))
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
    crate::log_invoke_debug!("repos_list");
    let Ok(entries) = std::fs::read_dir(repos_dir()) else { return Vec::new(); };
    let mut repos: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|e| {
            let data = std::fs::read_to_string(e.path()).ok()?;
            let s: RepoSettings = serde_json::from_str(&data).ok()?;
            if s.repo.is_empty() { None } else { Some(s.repo) }
        })
        .collect();
    repos.sort();
    repos
}

/// Return the hand-written JSON Schema for per-repo settings, for the Settings
/// window's JSON Forms renderer.
#[tauri::command]
pub fn repo_settings_schema() -> serde_json::Value {
    crate::log_invoke_debug!("repo_settings_schema");
    schema_value()
}

#[tauri::command]
pub fn repo_settings_get(repo: String) -> Result<RepoSettings, String> {
    crate::log_invoke_debug!("repo_settings_get", repo = %repo);
    load_validated(&repo)
}

#[tauri::command]
pub fn repo_settings_set(repo: String, mut settings: RepoSettings) -> Result<(), String> {
    crate::log_invoke!("repo_settings_set", repo = %repo);
    settings.repo = repo.clone();
    // Defense in depth: never persist a value the schema would reject on reload.
    let value = serde_json::to_value(&settings).map_err(|e| e.to_string())?;
    validate_against_schema(&value).map_err(|msg| format!("Invalid settings — {msg}"))?;
    save(&repo, &settings).map_err(|e| e.to_string())
}

/// Set (or clear) a repo's hide/snooze state. `hidden = None` unhides. Errors on
/// an unparseable existing file rather than clobbering it with defaults.
#[tauri::command]
pub fn repo_set_visibility(repo: String, hidden: Option<HideState>) -> Result<(), String> {
    crate::log_invoke!("repo_set_visibility", repo = %repo);
    let mut settings = load_validated(&repo)?;
    settings.hidden = hidden;
    save(&repo, &settings).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn repo_scan_env_files(checkout_dir: String) -> Vec<String> {
    crate::log_invoke!("repo_scan_env_files", checkout_dir = %checkout_dir);
    let base = Path::new(&checkout_dir);
    let mut abs = Vec::new();
    walk_env_files(base, 4, &mut abs);
    abs.iter()
        .filter_map(|p| Path::new(p).strip_prefix(base).ok())
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;

    /// The embedded schema string must be valid JSON, since `schema_value`
    /// `.expect()`s it at runtime.
    #[test]
    fn schema_parses() {
        let v = schema_value();
        assert!(v.get("properties").is_some(), "schema must declare properties");
    }

    /// Drift guard: the hand-written schema and the Rust struct must describe the
    /// same set of top-level fields, and every serialized `RepoSettings` must
    /// validate against the schema. Adding a field to one without the other fails
    /// here. This replaces the alignment that auto-generation would have given.
    #[test]
    fn schema_matches_struct() {
        let schema = schema_value();
        let schema_props: BTreeSet<String> = schema["properties"]
            .as_object()
            .expect("schema.properties is an object")
            .keys()
            .cloned()
            .collect();

        // A default instance and a fully-populated one — together they exercise
        // every field with both null and non-null values.
        let default = RepoSettings::default_for("acme/widget");
        let populated = RepoSettings {
            repo: "acme/widget".into(),
            identity_id: Some("id-123".into()),
            checkout_dir: Some("/home/u/src/widget".into()),
            worktree_prefix: Some("/home/u/src/work-".into()),
            env_files: vec![".env".into(), ".env.local".into()],
            hidden: Some(HideState { snooze_until: Some(1_717_372_800_000) }),
        };

        for instance in [&default, &populated] {
            let value = serde_json::to_value(instance).unwrap();
            // Same field set in both directions.
            let struct_keys: BTreeSet<String> =
                value.as_object().unwrap().keys().cloned().collect();
            assert_eq!(
                schema_props, struct_keys,
                "schema properties and serialized RepoSettings fields drifted apart"
            );
            // And it actually validates.
            validate_against_schema(&value)
                .unwrap_or_else(|e| panic!("serialized RepoSettings rejected by schema: {e}"));
        }
    }

    /// A minimal old file (only `checkout_dir` + `env_files`) still loads —
    /// backward compatible with files written before newer fields existed.
    #[test]
    fn minimal_old_file_passes() {
        let data = json!({
            "checkout_dir": "/home/u/src/widget",
            "env_files": [".env"]
        })
        .to_string();
        let settings = parse_and_validate(&data, "test").expect("minimal file should load");
        assert_eq!(settings.checkout_dir.as_deref(), Some("/home/u/src/widget"));
        assert_eq!(settings.env_files, vec![".env".to_string()]);
        assert!(settings.identity_id.is_none());
    }

    /// A wrong-typed field fails with a message naming that field.
    #[test]
    fn wrong_type_fails_with_field_message() {
        let data = json!({
            "checkout_dir": "/home/u/src/widget",
            "env_files": "not-an-array"
        })
        .to_string();
        let err = parse_and_validate(&data, "settings.json").expect_err("wrong type must fail");
        assert!(err.contains("failed validation"), "got: {err}");
        assert!(err.contains("env_files"), "message should name the field: {err}");
    }

    /// Unknown fields are tolerated (forward compatibility + hand-edited `$schema`).
    #[test]
    fn unknown_fields_tolerated() {
        let data = json!({
            "$schema": "./repo-settings.schema.json",
            "checkout_dir": "/home/u/src/widget",
            "env_files": [],
            "future_field": 42
        })
        .to_string();
        parse_and_validate(&data, "test").expect("unknown fields should be tolerated");
    }

    /// Malformed JSON fails loudly rather than silently falling back to defaults.
    #[test]
    fn malformed_json_fails() {
        let err = parse_and_validate("{ not json", "settings.json").expect_err("must fail");
        assert!(err.contains("not valid JSON"), "got: {err}");
    }
}
