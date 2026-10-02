use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

use crate::agent::Agent;

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

/// Per-repo overrides for the instructions mAIestro Code sends to the repo's agent. Each field
/// `None`/empty uses the built-in default (see `prompts.rs`). The runtime
/// context (idea, issue, diff) is appended automatically and is not part of
/// these overrides.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PromptOverrides {
    /// Instruction for drafting a GitHub issue from the user's idea.
    #[serde(default)]
    pub draft_issue: Option<String>,
    /// Instruction for summarizing an issue into a short workspace label.
    #[serde(default)]
    pub short_label: Option<String>,
    /// Instruction for drafting a pull request description from the diff.
    #[serde(default)]
    pub draft_pr: Option<String>,
}

/// Settings that apply to one agent, keyed by agent (only the repo's effective
/// agent's entry is used). Every agent has a drafting `prompt_model`; Claude
/// also has `remote_control`. Replaces the older flat `prompt_models` map,
/// migrated in `parse_and_validate`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentSettings {
    #[serde(default)]
    pub claude: ClaudeSettings,
    #[serde(default)]
    pub codex: AgentCommonSettings,
    #[serde(default)]
    pub antigravity: AgentCommonSettings,
    #[serde(default)]
    pub copilot: AgentCommonSettings,
}

impl AgentSettings {
    /// The drafting model configured for `agent` (unresolved — see
    /// `crate::prompts::model` for the schema-default fallback).
    pub fn prompt_model(&self, agent: Agent) -> &Option<String> {
        match agent {
            Agent::Claude => &self.claude.prompt_model,
            Agent::Codex => &self.codex.prompt_model,
            Agent::Antigravity => &self.antigravity.prompt_model,
            Agent::Copilot => &self.copilot.prompt_model,
        }
    }
}

/// Claude Code's per-repo settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClaudeSettings {
    /// The model for mAIestro Code's own drafting calls. `None`/empty uses the
    /// schema `default` (`haiku`). See `crate::prompts::model`.
    #[serde(default)]
    pub prompt_model: Option<String>,
    /// Launch the session with `--remote-control`. `None` uses the schema
    /// default (on) — see [`claude_remote_control`].
    #[serde(default)]
    pub remote_control: Option<bool>,
}

/// The per-repo settings of an agent that has only a drafting model. `None`/empty
/// uses that agent's schema `default` — a cheap Gemini Flash id for Antigravity,
/// and for Codex and Copilot no `--model` at all (Codex's own configured
/// default, Copilot's automatic model routing — its models depend on the plan).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentCommonSettings {
    #[serde(default)]
    pub prompt_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoSettings {
    /// Canonical "owner/name" string. Stored inside the file so repos_list can
    /// reconstruct the list without parsing the filename (which uses "-" and is ambiguous).
    #[serde(default)]
    pub repo: String,
    /// Identity used for credentials and agent spawning for this repo.
    pub identity_id: Option<String>,
    /// Absolute path to the local cloned repo directory (the primary checkout
    /// mAIestro Code creates worktrees from).
    pub cloned_repo_dir: Option<String>,
    /// Prefix for worktree locations. The full worktree path is
    /// `<worktree_prefix><workspace>/<repo>` (a string concatenation — the
    /// trailing segment is part of the directory name, not a path component).
    /// `None` falls back to `~/src/work-` at the use site, preserving the
    /// original hardcoded behavior. Tilde-expanded via `expand_tilde` in spawn.
    #[serde(default)]
    pub worktree_prefix: Option<String>,
    /// Env files relative to cloned_repo_dir, copied into a fresh worktree at spawn.
    #[serde(default)]
    pub env_files: Vec<String>,
    /// Shell commands to run in a freshly-created worktree, in order, before the
    /// editor opens (e.g. `pnpm install`). Empty by default. Run via the user's
    /// login shell so PATH and tool managers are available.
    #[serde(default)]
    pub post_spawn_commands: Vec<String>,
    /// Delete the session's branch on GitHub at teardown. `None` uses the schema
    /// default (on). Enables the delete but never overrides teardown's safety
    /// guard — see `spawn::remote_delete_skip_reason`.
    #[serde(default)]
    pub delete_remote_on_teardown: Option<bool>,
    /// Comment on the issue when a workspace is spawned for it. `None` uses the
    /// schema default (on). Does not affect issue assignment.
    #[serde(default)]
    pub comment_on_spawn: Option<bool>,
    /// The coding agent for this repo. `None` uses the global `agent` setting,
    /// then the app schema default — see [`effective_agent`].
    #[serde(default)]
    pub agent: Option<Agent>,
    /// Per-agent settings: each agent's drafting model (mAIestro Code's own
    /// draft_issue/short_label/draft_pr prompts, never the launched session) and
    /// Claude's `remote_control` launch flag. Replaces the older `prompt_models`
    /// and, before it, the single `prompt_model`, migrated in `parse_and_validate`.
    #[serde(default)]
    pub agent_settings: AgentSettings,
    /// Repo-level hide/snooze state. `None` = visible. A hidden repo hides its
    /// work items too. Per-work-item state lives on the session record, not here.
    #[serde(default)]
    pub hidden: Option<HideState>,
    /// Per-repo overrides for the AI prompt instructions. Defaults (all `None`)
    /// use the built-in prompts.
    #[serde(default)]
    pub prompts: PromptOverrides,
}

impl RepoSettings {
    pub(crate) fn default_for(repo: &str) -> Self {
        let name = repo.split('/').next_back().unwrap_or(repo);
        let cloned = crate::paths::home().join("src").join(name);
        Self {
            repo: repo.to_owned(),
            identity_id: None,
            cloned_repo_dir: Some(cloned.to_string_lossy().into_owned()),
            worktree_prefix: None,
            env_files: Vec::new(),
            post_spawn_commands: Vec::new(),
            delete_remote_on_teardown: None,
            comment_on_spawn: None,
            agent: None,
            agent_settings: AgentSettings::default(),
            hidden: None,
            prompts: PromptOverrides::default(),
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
    crate::schema::parse(SCHEMA_JSON, "repo-settings")
}

/// A string `default` from the embedded schema, addressed by JSON Pointer (e.g.
/// `/properties/worktree_prefix/default`). The schema is the single source of
/// truth for these defaults, so both backend resolution and the Settings form
/// read them from here rather than hardcoding. Returns `""` if absent.
pub fn schema_default(pointer: &str) -> String {
    crate::schema::default_str(&schema_value(), pointer)
}

/// A boolean `default` from the embedded schema, addressed by JSON Pointer (e.g.
/// `/properties/comment_on_spawn/default`). The boolean sibling of
/// [`schema_default`], used to resolve the `Option<bool>` settings whose `None`
/// means "use the default" — so the default itself lives only in the schema.
pub fn schema_default_bool(pointer: &str) -> bool {
    crate::schema::default_bool(&schema_value(), pointer)
}

/// Resolve an `Option<bool>` setting against its schema `default`, which is the
/// only place the default is declared. `pointer` addresses that default.
pub fn bool_or_default(configured: Option<bool>, pointer: &str) -> bool {
    configured.unwrap_or_else(|| schema_default_bool(pointer))
}

/// The agent a repo uses: its own `agent`, else the global `agent` setting, else
/// the app schema `default`. **Every** AI call site (drafting, the PR draft, the
/// health probe) and a fresh spawn resolve the agent through here — none of them
/// assumes `claude`. A spawned session records the result, so reopening reads
/// the record instead (see `sessions::Session::agent`).
pub fn effective_agent(settings: &RepoSettings) -> Agent {
    settings.agent.unwrap_or_else(crate::app_settings::agent)
}

/// Whether the Claude session launches with `--remote-control`: the repo's
/// `agent_settings.claude.remote_control`, else its schema default (on).
pub fn claude_remote_control(settings: &RepoSettings) -> bool {
    bool_or_default(
        settings.agent_settings.claude.remote_control,
        "/properties/agent_settings/properties/claude/properties/remote_control/default",
    )
}

/// Validate a settings JSON value against the embedded schema. Returns a message
/// naming the failing field(s) on error.
fn validate_against_schema(value: &serde_json::Value) -> Result<(), String> {
    crate::schema::validate(&schema_value(), value)
}

// ── Storage ───────────────────────────────────────────────────────────────────

fn repos_dir() -> PathBuf {
    crate::paths::maiestro_dir("repos")
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
    let mut value: serde_json::Value = serde_json::from_str(data)
        .map_err(|e| format!("{label} is not valid JSON: {e}"))?;
    migrate_prompt_model(&mut value);
    migrate_prompt_models(&mut value);
    validate_against_schema(&value).map_err(|msg| format!("{label} failed validation — {msg}"))?;
    serde_json::from_value(value).map_err(|e| format!("{label} does not match RepoSettings: {e}"))
}

/// Carry a pre-#162 `prompt_model` (a `claude --model` alias) over to
/// `prompt_models.claude`, so a custom drafting model isn't lost when the field
/// was split per agent. An explicit `prompt_models.claude` wins. The old key is
/// dropped from the value; [`migrate_prompt_models`] then carries the result on
/// to `agent_settings`.
fn migrate_prompt_model(value: &mut serde_json::Value) {
    let Some(obj) = value.as_object_mut() else { return };
    let Some(old) = obj.remove("prompt_model") else { return };
    let Some(old) = old.as_str().map(str::trim).filter(|s| !s.is_empty()) else { return };
    let models = obj.entry("prompt_models").or_insert(serde_json::Value::Null);
    if !models.is_object() {
        *models = serde_json::json!({});
    }
    if models["claude"].as_str().is_none_or(|s| s.trim().is_empty()) {
        models["claude"] = serde_json::Value::String(old.to_string());
    }
}

/// Carry a pre-#209 `prompt_models.<agent>` over to
/// `agent_settings.<agent>.prompt_model`, so custom drafting models survive the
/// move into per-agent settings. Runs after [`migrate_prompt_model`], so an even
/// older `prompt_model` reaches the new shape too. An explicit new-shape value
/// wins, and blank or non-string entries are dropped. The old key is removed
/// from the value, so the next save writes only the new shape.
fn migrate_prompt_models(value: &mut serde_json::Value) {
    let Some(obj) = value.as_object_mut() else { return };
    let Some(old) = obj.remove("prompt_models") else { return };
    let Some(old) = old.as_object() else { return };
    for (agent, model) in old {
        let Some(model) = model.as_str().map(str::trim).filter(|s| !s.is_empty()) else { continue };
        let settings = obj.entry("agent_settings").or_insert(serde_json::Value::Null);
        if !settings.is_object() {
            *settings = serde_json::json!({});
        }
        let entry = &mut settings[agent.as_str()];
        if !entry.is_object() {
            *entry = serde_json::json!({});
        }
        if entry["prompt_model"].as_str().is_none_or(|s| s.trim().is_empty()) {
            entry["prompt_model"] = serde_json::Value::String(model.to_string());
        }
    }
}

fn save(repo: &str, settings: &RepoSettings) -> std::io::Result<()> {
    let dir = repos_dir();
    std::fs::create_dir_all(&dir)?;
    let data = serde_json::to_string_pretty(settings).unwrap();
    crate::paths::write_atomic(&settings_path(repo), data.as_bytes())
}

/// Clear `identity_id` from every repo settings file that references the given
/// identity, so removing an identity leaves repos reading "no identity assigned"
/// instead of pointing at a ghost. Best-effort: a malformed or unwritable file is
/// skipped with a warning rather than failing the identity removal. Edits the raw
/// JSON value (not the `RepoSettings` struct) so unknown fields written by a
/// newer app version survive the rewrite.
pub fn clear_identity_references(identity_id: &str) {
    let Ok(entries) = std::fs::read_dir(repos_dir()) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Ok(data) = std::fs::read_to_string(&path) else { continue };
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&data) else {
            tracing::warn!(path = %path.display(), "skipping malformed repo settings while clearing identity reference");
            continue;
        };
        if value.get("identity_id").and_then(|v| v.as_str()) != Some(identity_id) {
            continue;
        }
        value["identity_id"] = serde_json::Value::Null;
        let out = serde_json::to_string_pretty(&value).expect("Value is always serializable");
        if let Err(e) = crate::paths::write_atomic(&path, out.as_bytes()) {
            tracing::warn!(error = %e, path = %path.display(), "failed to clear identity reference");
        }
    }
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

/// Untrack a repo by deleting its settings file. Idempotent — a missing file is
/// already the goal state ("not tracked"). Worktrees, checkouts, and session
/// records are untouched; sessions for the repo simply stop rendering because
/// the popover only shows sessions under tracked repos.
#[tauri::command]
pub fn repo_remove(repo: String) -> Result<(), String> {
    crate::log_invoke!("repo_remove", repo = %repo);
    match std::fs::remove_file(settings_path(&repo)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Failed to remove repo settings: {e}")),
    }
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
pub fn repo_scan_env_files(cloned_repo_dir: String) -> Vec<String> {
    crate::log_invoke!("repo_scan_env_files", cloned_repo_dir = %cloned_repo_dir);
    // Tilde-expand like every other consumer of this setting (spawn, health).
    let base = crate::paths::expand_tilde(&cloned_repo_dir);
    let mut abs = Vec::new();
    walk_env_files(&base, 4, &mut abs);
    abs.iter()
        .filter_map(|p| Path::new(p).strip_prefix(&base).ok())
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

    /// Each `prompt_model`'s help text that names its default ("…the default
    /// (haiku)") so the Settings form doesn't make the user go look it up holds a
    /// second copy of the value, so pin it: changing the `default` without
    /// rewording the `description` fails here rather than shipping a form that lies.
    #[test]
    fn prompt_model_help_names_its_default() {
        let schema = schema_value();
        for agent in ["claude", "antigravity"] {
            let prop = &schema["properties"]["agent_settings"]["properties"][agent]["properties"]["prompt_model"];
            let default = prop["default"].as_str().expect("prompt_model declares a default");
            let description = prop["description"].as_str().expect("prompt_model is described");
            assert!(
                description.contains(&format!("({default})")),
                "{agent}.prompt_model's description must name its default `{default}`, but reads: {description}"
            );
        }
    }

    /// A pre-#162 file's custom `prompt_model` reaches
    /// `agent_settings.claude.prompt_model` through both migrations; an explicit
    /// newer-shape value wins; a blank one is just dropped. Every other field is
    /// untouched.
    #[test]
    fn legacy_prompt_model_migrates_to_claude_entry() {
        let s = parse_and_validate(&json!({ "repo": "a/b", "prompt_model": "sonnet" }).to_string(), "t").unwrap();
        assert_eq!(s.agent_settings.claude.prompt_model.as_deref(), Some("sonnet"));
        assert!(s.agent_settings.codex.prompt_model.is_none());
        assert_eq!(s.repo, "a/b");

        let s = parse_and_validate(
            &json!({ "prompt_model": "sonnet", "prompt_models": { "claude": "opus" } }).to_string(),
            "t",
        )
        .unwrap();
        assert_eq!(s.agent_settings.claude.prompt_model.as_deref(), Some("opus"));

        let s = parse_and_validate(&json!({ "prompt_model": "" }).to_string(), "t").unwrap();
        assert!(s.agent_settings.claude.prompt_model.is_none());

        // The migrated shape is what gets written back.
        let v = serde_json::to_value(parse_and_validate(&json!({ "prompt_model": "sonnet" }).to_string(), "t").unwrap()).unwrap();
        assert!(v.get("prompt_model").is_none());
        assert!(v.get("prompt_models").is_none());
        assert_eq!(v["agent_settings"]["claude"]["prompt_model"], "sonnet");
    }

    /// A pre-#209 `prompt_models` map moves under `agent_settings.<agent>`
    /// entry by entry; an explicit `agent_settings` value wins, other
    /// `agent_settings` fields survive, and blank or null entries are dropped.
    #[test]
    fn prompt_models_migrate_to_agent_settings() {
        let s = parse_and_validate(
            &json!({ "prompt_models": { "claude": "sonnet", "codex": "gpt-5-codex", "antigravity": " ", "copilot": null } })
                .to_string(),
            "t",
        )
        .unwrap();
        assert_eq!(s.agent_settings.claude.prompt_model.as_deref(), Some("sonnet"));
        assert_eq!(s.agent_settings.codex.prompt_model.as_deref(), Some("gpt-5-codex"));
        assert!(s.agent_settings.antigravity.prompt_model.is_none());
        assert!(s.agent_settings.copilot.prompt_model.is_none());

        let s = parse_and_validate(
            &json!({
                "prompt_models": { "claude": "sonnet", "codex": "gpt-5-codex" },
                "agent_settings": { "claude": { "prompt_model": "opus", "remote_control": false } }
            })
            .to_string(),
            "t",
        )
        .unwrap();
        assert_eq!(s.agent_settings.claude.prompt_model.as_deref(), Some("opus"));
        assert_eq!(s.agent_settings.claude.remote_control, Some(false));
        assert_eq!(s.agent_settings.codex.prompt_model.as_deref(), Some("gpt-5-codex"));

        // A null `prompt_models` (the old schema allowed it) is just dropped.
        let s = parse_and_validate(&json!({ "prompt_models": null }).to_string(), "t").unwrap();
        assert!(s.agent_settings.claude.prompt_model.is_none());

        let v = serde_json::to_value(s).unwrap();
        assert!(v.get("prompt_models").is_none());
    }

    /// Remote control is on unless the repo turns it off.
    #[test]
    fn claude_remote_control_defaults_on() {
        let mut s = RepoSettings::default_for("a/b");
        assert!(claude_remote_control(&s), "schema default");
        s.agent_settings.claude.remote_control = Some(false);
        assert!(!claude_remote_control(&s));
        let s = parse_and_validate(&json!({ "agent_settings": { "claude": { "remote_control": null } } }).to_string(), "t").unwrap();
        assert!(claude_remote_control(&s), "null uses the default");
    }

    /// The repo's own agent wins over the global one; unset falls through.
    #[test]
    fn effective_agent_prefers_the_repo_value() {
        let _home = TempHome::new();
        let mut s = RepoSettings::default_for("a/b");
        assert_eq!(effective_agent(&s), Agent::Claude, "schema default");
        crate::app_settings::update(|a| a.agent = Some(Agent::Codex)).unwrap();
        assert_eq!(effective_agent(&s), Agent::Codex, "global setting");
        s.agent = Some(Agent::Claude);
        assert_eq!(effective_agent(&s), Agent::Claude, "repo override");
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
            cloned_repo_dir: Some("/home/u/src/widget".into()),
            worktree_prefix: Some("/home/u/src/work-".into()),
            env_files: vec![".env".into(), ".env.local".into()],
            post_spawn_commands: vec!["pnpm install".into()],
            delete_remote_on_teardown: Some(false),
            comment_on_spawn: Some(false),
            agent: Some(Agent::Antigravity),
            agent_settings: AgentSettings {
                claude: ClaudeSettings { prompt_model: Some("sonnet".into()), remote_control: Some(false) },
                codex: AgentCommonSettings { prompt_model: Some("gpt-5-codex".into()) },
                antigravity: AgentCommonSettings { prompt_model: Some("gemini-3.1-pro-low".into()) },
                copilot: AgentCommonSettings { prompt_model: Some("gpt-5-mini".into()) },
            },
            hidden: Some(HideState { snooze_until: Some(1_717_372_800_000) }),
            prompts: PromptOverrides {
                draft_issue: Some("Custom issue instruction".into()),
                short_label: None,
                draft_pr: Some("Custom PR instruction".into()),
            },
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

    /// A minimal file (only `cloned_repo_dir` + `env_files`) still loads —
    /// backward compatible with files written before newer fields existed.
    #[test]
    fn minimal_old_file_passes() {
        let data = json!({
            "cloned_repo_dir": "/home/u/src/widget",
            "env_files": [".env"]
        })
        .to_string();
        let settings = parse_and_validate(&data, "test").expect("minimal file should load");
        assert_eq!(settings.cloned_repo_dir.as_deref(), Some("/home/u/src/widget"));
        assert_eq!(settings.env_files, vec![".env".to_string()]);
        assert!(settings.identity_id.is_none());
    }

    /// Every field is schema-optional (no `required` array), so a hand-written
    /// file that omits any of them — including the non-Option `env_files` — must
    /// deserialize, not fail with a "missing field" error after passing schema
    /// validation.
    #[test]
    fn schema_valid_file_without_env_files_loads() {
        let data = json!({
            "repo": "acme/widget",
            "cloned_repo_dir": "~/src/widget"
        })
        .to_string();
        let settings = parse_and_validate(&data, "test").expect("file without env_files should load");
        assert!(settings.env_files.is_empty());
    }

    /// A wrong-typed field fails with a message naming that field.
    #[test]
    fn wrong_type_fails_with_field_message() {
        let data = json!({
            "cloned_repo_dir": "/home/u/src/widget",
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
            "cloned_repo_dir": "/home/u/src/widget",
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

    // ── Filesystem-level tests (MAIESTRO_HOME-injected temp root) ───────────────
    //
    // These exercise the real load → validate → atomic-write cycle against files,
    // which `parse_and_validate`'s pure tests above can't reach. `TempHome`
    // redirects `maiestro_dir` at a tempdir, so nothing touches `~/.maiestro`.

    use crate::testutil::TempHome;

    /// `load_validated`, outcome 1: a missing file yields defaults, not an error.
    #[test]
    fn get_missing_file_returns_defaults() {
        let _home = TempHome::new();
        let settings = repo_settings_get("acme/widget".into()).expect("missing file → defaults");
        assert_eq!(settings.repo, "acme/widget");
        assert!(settings.identity_id.is_none());
        assert!(settings.env_files.is_empty());
    }

    /// `load_validated`, outcome 3 (valid), via a real set → get round-trip. Also
    /// proves `repo_settings_set` stamps the repo and the file lands on disk.
    #[test]
    fn set_then_get_roundtrips_through_file() {
        let home = TempHome::new();
        let mut settings = RepoSettings::default_for("acme/widget");
        settings.cloned_repo_dir = Some("~/src/widget".into());
        settings.env_files = vec![".env".into(), ".env.local".into()];
        settings.identity_id = Some("work".into());

        repo_settings_set("acme/widget".into(), settings).expect("set should persist");

        // The file exists under the injected root with the mangled name.
        assert!(home.join("repos/acme-widget.json").exists(), "settings file should be written");

        let loaded = repo_settings_get("acme/widget".into()).expect("get should load");
        assert_eq!(loaded.cloned_repo_dir.as_deref(), Some("~/src/widget"));
        assert_eq!(loaded.env_files, vec![".env".to_string(), ".env.local".into()]);
        assert_eq!(loaded.identity_id.as_deref(), Some("work"));
        assert_eq!(loaded.repo, "acme/widget", "set must stamp the repo field");
    }

    /// `load_validated`, outcome 2: a present-but-invalid file is a loud error
    /// naming the file and field — never a silent fall back to defaults.
    #[test]
    fn get_invalid_file_errors_loudly() {
        let home = TempHome::new();
        std::fs::create_dir_all(home.join("repos")).unwrap();
        std::fs::write(
            home.join("repos/acme-widget.json"),
            json!({ "env_files": "not-an-array" }).to_string(),
        )
        .unwrap();

        let err = repo_settings_get("acme/widget".into()).expect_err("invalid file must error");
        assert!(err.contains("env_files"), "error should name the field: {err}");
    }

    /// `repo_set_visibility` errors on an unparseable file rather than clobbering
    /// it with defaults (issue #101 behavior).
    #[test]
    fn set_visibility_refuses_to_clobber_bad_file() {
        let home = TempHome::new();
        std::fs::create_dir_all(home.join("repos")).unwrap();
        let path = home.join("repos/acme-widget.json");
        std::fs::write(&path, "{ not json").unwrap();

        let err = repo_set_visibility("acme/widget".into(), None).expect_err("must refuse");
        assert!(err.contains("not valid JSON"), "got: {err}");
        // The bad file is left intact, not overwritten with defaults.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }

    /// `repo_remove` is idempotent — removing a never-tracked repo is Ok.
    #[test]
    fn remove_missing_is_ok() {
        let _home = TempHome::new();
        repo_remove("never/tracked".into()).expect("removing a missing repo is idempotent");
    }
}
