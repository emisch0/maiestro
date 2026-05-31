use std::collections::HashMap;
use crate::plugin::{CredentialTypeInfo, Plugin, ResolvedCredential};
use crate::credentials::{CredentialError, CredentialScope, CredentialStore};

const TYPES: &[CredentialTypeInfo] = &[CredentialTypeInfo {
    type_id: "anthropic_key",
    display_name: "Anthropic API Key",
    env_var: "ANTHROPIC_API_KEY",
    description: "API key for Claude via the Anthropic API.",
}];

pub struct AnthropicPlugin;

impl Plugin for AnthropicPlugin {
    fn id(&self) -> &str { "anthropic" }

    fn credential_types(&self) -> &[CredentialTypeInfo] { TYPES }

    fn on_spawn_env(&self, env: &mut HashMap<String, String>, resolved: &[ResolvedCredential]) {
        let mode = resolved.iter()
            .find(|c| c.type_id == "anthropic_auth_mode")
            .map(|c| c.value.as_str())
            .unwrap_or("api_key");

        if mode == "claude_session" {
            // The claude CLI reads its OAuth session from ~/.claude/ via $HOME.
            // Ensure $HOME is present in the spawn env; do not inject ANTHROPIC_API_KEY.
            return;
        }

        if let Some(cred) = resolved.iter().find(|c| c.type_id == "anthropic_key") {
            env.insert("ANTHROPIC_API_KEY".into(), cred.value.clone());
        }
    }
}

// ── Tauri commands ─────────────────────────────────────────────────────────────

#[tauri::command]
pub fn anthropic_auth_mode_get(scope: CredentialScope) -> String {
    CredentialStore::get("anthropic_auth_mode", &scope)
        .unwrap_or_else(|_| "api_key".into())
}

#[tauri::command]
pub fn anthropic_auth_mode_set(scope: CredentialScope, mode: String) -> Result<(), CredentialError> {
    CredentialStore::set("anthropic_auth_mode", &scope, &mode)
}
