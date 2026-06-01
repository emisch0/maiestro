use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn identities_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".maiestro/identities.json")
}

#[derive(Default, Serialize, Deserialize)]
struct Store {
    identities: Vec<String>,
    #[serde(default)]
    default: Option<String>,
}

fn load_store() -> Store {
    let data = std::fs::read_to_string(identities_path()).unwrap_or_default();
    if let Ok(store) = serde_json::from_str::<Store>(&data) {
        return store;
    }
    // Legacy format: a bare JSON array of identity IDs, no default pointer.
    if let Ok(identities) = serde_json::from_str::<Vec<String>>(&data) {
        let default = identities.first().cloned();
        return Store { identities, default };
    }
    Store::default()
}

fn save_store(store: &Store) -> std::io::Result<()> {
    let path = identities_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(store).unwrap();
    std::fs::write(path, data)
}

/// Register an identity ID in the persistent list. Best-effort: silently ignores IO errors.
/// The first identity ever registered also becomes the default.
pub fn register(identity_id: &str) {
    let mut store = load_store();
    let mut changed = false;
    if !store.identities.iter().any(|i| i == identity_id) {
        store.identities.push(identity_id.to_owned());
        store.identities.sort();
        changed = true;
    }
    if store.default.is_none() {
        store.default = Some(identity_id.to_owned());
        changed = true;
    }
    if changed {
        let _ = save_store(&store);
    }
}

#[tauri::command]
pub fn identities_list() -> Vec<String> {
    load_store().identities
}

/// The identity to pre-select on launch. Falls back to the first known identity
/// when the stored default is missing or points at an identity that no longer exists.
#[tauri::command]
pub fn identities_get_default() -> Option<String> {
    let store = load_store();
    match &store.default {
        Some(d) if store.identities.iter().any(|i| i == d) => Some(d.clone()),
        _ => store.identities.first().cloned(),
    }
}

#[tauri::command]
pub fn identities_set_default(identity_id: String) -> Result<(), String> {
    let mut store = load_store();
    if !store.identities.iter().any(|i| i == &identity_id) {
        store.identities.push(identity_id.clone());
        store.identities.sort();
    }
    store.default = Some(identity_id);
    save_store(&store).map_err(|e| e.to_string())
}
