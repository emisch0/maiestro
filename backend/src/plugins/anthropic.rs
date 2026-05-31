use std::collections::HashMap;
use crate::plugin::{CredentialTypeInfo, Plugin, ResolvedCredential};

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
        if let Some(cred) = resolved.iter().find(|c| c.type_id == "anthropic_key") {
            env.insert("ANTHROPIC_API_KEY".into(), cred.value.clone());
        }
    }
}
