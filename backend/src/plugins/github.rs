use std::collections::HashMap;
use crate::plugin::{CredentialTypeInfo, Plugin, ResolvedCredential};

const TYPES: &[CredentialTypeInfo] = &[CredentialTypeInfo {
    type_id: "github_token",
    display_name: "GitHub Token",
    env_var: "GITHUB_TOKEN",
    description: "Personal access token for GitHub API calls and git operations over HTTPS.",
}];

pub struct GitHubPlugin;

impl Plugin for GitHubPlugin {
    fn id(&self) -> &str { "github" }

    fn credential_types(&self) -> &[CredentialTypeInfo] { TYPES }

    fn on_spawn_env(&self, env: &mut HashMap<String, String>, resolved: &[ResolvedCredential]) {
        if let Some(cred) = resolved.iter().find(|c| c.type_id == "github_token") {
            env.insert("GITHUB_TOKEN".into(), cred.value.clone());
        }
    }
}
