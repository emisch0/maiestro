use std::path::PathBuf;

fn identities_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".maiestro/identities.json")
}

fn load_list() -> Vec<String> {
    let data = std::fs::read_to_string(identities_path()).unwrap_or_default();
    serde_json::from_str::<Vec<String>>(&data).unwrap_or_default()
}

fn save_list(list: &[String]) -> std::io::Result<()> {
    let path = identities_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(list).unwrap();
    std::fs::write(path, data)
}

/// Register an identity ID in the persistent list. Best-effort: silently ignores IO errors.
pub fn register(identity_id: &str) {
    let mut list = load_list();
    if !list.contains(&identity_id.to_owned()) {
        list.push(identity_id.to_owned());
        list.sort();
        let _ = save_list(&list);
    }
}

#[tauri::command]
pub fn identities_list() -> Vec<String> {
    load_list()
}
