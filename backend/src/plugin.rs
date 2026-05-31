use std::collections::HashMap;
use serde::Serialize;

/// Metadata a plugin declares for a credential type it owns.
#[derive(Debug, Clone)]
pub struct CredentialTypeInfo {
    /// Stable identifier used as the `type_id` in the credential store (e.g. `"github_token"`).
    pub type_id: &'static str,
    pub display_name: &'static str,
    /// The env var injected into subprocesses when this credential is resolved.
    pub env_var: &'static str,
    pub description: &'static str,
}

/// A credential value already fetched from Keychain for a specific spawn context.
pub struct ResolvedCredential {
    pub type_id: String,
    pub value: String,
}

/// A plugin contributes one or more credential types and knows how to inject them
/// into a subprocess environment at spawn time.
pub trait Plugin: Send + Sync {
    fn id(&self) -> &str;
    fn credential_types(&self) -> &[CredentialTypeInfo];
    fn on_spawn_env(&self, env: &mut HashMap<String, String>, resolved: &[ResolvedCredential]);
}

/// Registry of all active plugins. Constructed at app startup and stored as
/// immutable Tauri managed state — no mutex needed after init.
pub struct PluginRegistry {
    plugins: Vec<Box<dyn Plugin>>,
}

impl PluginRegistry {
    pub fn builder() -> PluginRegistryBuilder {
        PluginRegistryBuilder { plugins: Vec::new() }
    }

    pub fn all_credential_types(&self) -> impl Iterator<Item = &CredentialTypeInfo> {
        self.plugins.iter().flat_map(|p| p.credential_types())
    }

    /// Build the subprocess env by asking every plugin to inject its credentials.
    pub fn build_spawn_env(&self, resolved: &[ResolvedCredential]) -> HashMap<String, String> {
        let mut env = HashMap::new();
        for plugin in &self.plugins {
            plugin.on_spawn_env(&mut env, resolved);
        }
        env
    }
}

pub struct PluginRegistryBuilder {
    plugins: Vec<Box<dyn Plugin>>,
}

impl PluginRegistryBuilder {
    pub fn register(mut self, plugin: impl Plugin + 'static) -> Self {
        self.plugins.push(Box::new(plugin));
        self
    }

    pub fn build(self) -> PluginRegistry {
        PluginRegistry { plugins: self.plugins }
    }
}

// ── Tauri command ─────────────────────────────────────────────────────────────

/// Serializable DTO for the JS bridge.
#[derive(Serialize)]
pub struct CredentialTypeDto {
    pub type_id: &'static str,
    pub display_name: &'static str,
    pub env_var: &'static str,
    pub description: &'static str,
}

#[tauri::command]
pub fn plugins_list_credential_types(
    registry: tauri::State<'_, PluginRegistry>,
) -> Vec<CredentialTypeDto> {
    registry
        .all_credential_types()
        .map(|t| CredentialTypeDto {
            type_id: t.type_id,
            display_name: t.display_name,
            env_var: t.env_var,
            description: t.description,
        })
        .collect()
}
