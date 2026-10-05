//! Commands the user has approved to run without asking (issue #225).
//!
//! Approvals live in `~/.maiestro/approvals/`, one file per scope, and never in
//! the settings files: no settings writer (the Settings form's autosave,
//! `repo_set_agent`, `app_settings::update`) can erase or change them.
//! - `approvals/<owner>-<name>.json` — one repo, named like its settings file;
//! - `approvals/global.json` — every repo.
//!
//! The only write is [`add`], which appends. Nothing in the app removes or
//! rewrites an entry; revoking an approval means editing the file by hand. The
//! format is specified by the hand-written `backend/schemas/approvals.schema.json`
//! (the `schema_matches_struct` test keeps [`Approvals`] in sync with it).

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// One approvals file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Approvals {
    /// Post-spawn commands approved to run without the confirmation dialog,
    /// matched by their exact trimmed text.
    #[serde(default)]
    pub post_spawn_commands: Vec<String>,
}

/// Which approvals file: one repo's (`owner/name`) or the all-repos one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope<'a> {
    Repo(&'a str),
    Global,
}

impl Scope<'_> {
    fn label(&self) -> &str {
        match self {
            Scope::Repo(repo) => repo,
            Scope::Global => "global",
        }
    }
}

// ── Schema ──────────────────────────────────────────────────────────────────────

const SCHEMA_JSON: &str = include_str!("../schemas/approvals.schema.json");

fn schema_value() -> serde_json::Value {
    crate::schema::parse(SCHEMA_JSON, "approvals")
}

fn validate_against_schema(value: &serde_json::Value) -> Result<(), String> {
    crate::schema::validate(&schema_value(), value)
}

// ── Storage ─────────────────────────────────────────────────────────────────────

fn approvals_dir() -> PathBuf {
    crate::paths::maiestro_dir("approvals")
}

/// The file for `scope`. A repo file always contains the `<owner>-` prefix, so
/// no repo can collide with `global.json`.
pub fn path(scope: Scope) -> PathBuf {
    match scope {
        Scope::Repo(repo) => approvals_dir().join(crate::repo_settings::file_name(repo)),
        Scope::Global => approvals_dir().join("global.json"),
    }
}

/// Read and validate a scope's file as a raw JSON value, so [`add`] can append
/// without dropping fields written by a newer app version. Missing file → an
/// empty object; present but unparsable or failing the schema → an error naming
/// the file.
fn load_value(scope: Scope) -> Result<serde_json::Value, String> {
    let path = path(scope);
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(serde_json::json!({})),
        Err(e) => return Err(format!("Failed to read {}: {e}", path.display())),
    };
    let label = path.display().to_string();
    let value: serde_json::Value =
        serde_json::from_str(&data).map_err(|e| format!("{label} is not valid JSON: {e}"))?;
    validate_against_schema(&value).map_err(|msg| format!("{label} failed validation — {msg}"))?;
    Ok(value)
}

/// Load a scope's approvals strictly: missing → empty, unreadable → `Err`.
pub fn load(scope: Scope) -> Result<Approvals, String> {
    let value = load_value(scope)?;
    serde_json::from_value(value).map_err(|e| format!("{} does not match Approvals: {e}", path(scope).display()))
}

/// The post-spawn commands approved in `scope`. **Fails closed**: an unreadable
/// file logs at `error` and reads as no approvals, so every spawn with commands
/// asks again rather than trusting a file we can't read.
pub fn approved_post_spawn_commands(scope: Scope) -> Vec<String> {
    match load(scope) {
        Ok(a) => a.post_spawn_commands,
        Err(e) => {
            tracing::error!(error = %e, scope = scope.label(), "could not read approvals; treating as none approved");
            Vec::new()
        }
    }
}

/// Serializes [`add`]'s read-modify-write, so two spawns approving at once can't
/// lose one another's entries.
static APPROVALS_LOCK: Mutex<()> = Mutex::new(());

/// Append `commands` (trimmed, blanks dropped, deduplicated) to a scope's
/// approved post-spawn commands. The one writer: it only ever adds, keeps every
/// existing entry and unknown field, and writes atomically (temp + rename).
/// **Never writes over a file it can't read** — it logs at `error` and returns
/// the error, leaving the file byte-for-byte as it was. A missing file (or
/// directory) is created.
pub fn add(scope: Scope, commands: &[String]) -> Result<(), String> {
    let _guard = APPROVALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut value = load_value(scope).inspect_err(|e| {
        tracing::error!(error = %e, scope = scope.label(), "refusing to write approvals over an unreadable file; fix or delete it");
    })?;

    let list = value
        .as_object_mut()
        .expect("schema guarantees an object")
        .entry("post_spawn_commands")
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .expect("schema guarantees an array");
    let mut added = 0;
    for cmd in commands.iter().map(|c| c.trim()).filter(|c| !c.is_empty()) {
        if !list.iter().any(|v| v.as_str() == Some(cmd)) {
            list.push(serde_json::Value::String(cmd.to_string()));
            added += 1;
        }
    }
    if added == 0 {
        return Ok(());
    }

    let path = path(scope);
    std::fs::create_dir_all(approvals_dir()).map_err(|e| format!("could not create {}: {e}", approvals_dir().display()))?;
    let data = serde_json::to_string_pretty(&value).expect("Value is always serializable");
    crate::paths::write_atomic(&path, data.as_bytes()).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    tracing::info!(scope = scope.label(), added, "approved post-spawn commands");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;

    fn cmds(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// Adding a field to the schema without the struct (or vice versa) fails here.
    #[test]
    fn schema_matches_struct() {
        let schema = schema_value();
        let schema_props: BTreeSet<String> = schema["properties"].as_object().unwrap().keys().cloned().collect();
        let populated = Approvals { post_spawn_commands: cmds(&["pnpm install"]) };
        let value = serde_json::to_value(&populated).unwrap();
        let struct_keys: BTreeSet<String> = value.as_object().unwrap().keys().cloned().collect();
        assert_eq!(schema_props, struct_keys, "schema properties and Approvals fields drifted apart");
        validate_against_schema(&value).unwrap();
        validate_against_schema(&serde_json::to_value(Approvals::default()).unwrap()).unwrap();
    }

    #[test]
    fn file_names_follow_the_scope() {
        let home = crate::testutil::TempHome::new();
        assert_eq!(path(Scope::Repo("yanokamay-org/maiestro")), home.join("approvals").join("yanokamay-org-maiestro.json"));
        assert_eq!(path(Scope::Global), home.join("approvals").join("global.json"));
    }

    #[test]
    fn missing_file_means_no_approvals() {
        let _home = crate::testutil::TempHome::new();
        assert_eq!(load(Scope::Global).unwrap(), Approvals::default());
        assert!(approved_post_spawn_commands(Scope::Repo("acme/widget")).is_empty());
    }

    #[test]
    fn add_creates_the_file_trims_and_deduplicates() {
        let _home = crate::testutil::TempHome::new();
        add(Scope::Repo("acme/widget"), &cmds(&["  pnpm install ", "", "pnpm install", "cargo fetch"])).unwrap();
        add(Scope::Repo("acme/widget"), &cmds(&["cargo fetch"])).unwrap();
        assert_eq!(approved_post_spawn_commands(Scope::Repo("acme/widget")), cmds(&["pnpm install", "cargo fetch"]));
        assert!(approved_post_spawn_commands(Scope::Global).is_empty(), "scopes are separate files");
    }

    #[test]
    fn add_keeps_existing_entries_and_unknown_fields() {
        let home = crate::testutil::TempHome::new();
        std::fs::create_dir_all(home.join("approvals")).unwrap();
        let file = home.join("approvals").join("global.json");
        std::fs::write(&file, r#"{ "post_spawn_commands": ["make"], "future_field": 1 }"#).unwrap();
        add(Scope::Global, &cmds(&["pnpm install"])).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(saved, json!({ "post_spawn_commands": ["make", "pnpm install"], "future_field": 1 }));
    }

    #[test]
    fn add_refuses_to_overwrite_a_corrupt_file() {
        let home = crate::testutil::TempHome::new();
        std::fs::create_dir_all(home.join("approvals")).unwrap();
        let file = home.join("approvals").join("acme-widget.json");
        for broken in [r#"{ "post_spawn_commands": ["make"], }"#, r#"{ "post_spawn_commands": "make" }"#] {
            std::fs::write(&file, broken).unwrap();
            assert!(add(Scope::Repo("acme/widget"), &cmds(&["pnpm install"])).is_err());
            assert_eq!(std::fs::read_to_string(&file).unwrap(), broken, "file must be untouched");
            assert!(approved_post_spawn_commands(Scope::Repo("acme/widget")).is_empty(), "fails closed");
        }
        // The other scope still works.
        add(Scope::Global, &cmds(&["pnpm install"])).unwrap();
        assert_eq!(approved_post_spawn_commands(Scope::Global), cmds(&["pnpm install"]));
    }
}
