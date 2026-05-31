use std::collections::HashMap;
use crate::credentials::{CredentialScope, CredentialStore};
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

// ── Tauri commands ─────────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct RepoItem {
    pub full_name: String,
    pub private: bool,
    pub description: Option<String>,
}

#[tauri::command]
pub async fn github_list_repos(identity_id: String) -> Result<Vec<RepoItem>, String> {
    let scope = CredentialScope::Identity { identity_id };
    let token = CredentialStore::get("github_token", &scope)
        .map_err(|_| "No GitHub token found for this profile. Set one in the Profile tab first.".to_string())?;

    let client = reqwest::Client::builder()
        .user_agent("maiestro/0.1")
        .build()
        .map_err(|e| e.to_string())?;

    let mut repos = Vec::new();
    let mut page: u32 = 1;

    loop {
        let resp = client
            .get("https://api.github.com/user/repos")
            .query(&[
                ("per_page", "100"),
                ("page", &page.to_string()),
                ("sort", "pushed"),
                ("affiliation", "owner,collaborator,organization_member"),
            ])
            .bearer_auth(&token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let msg = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["message"].as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| format!("HTTP {}", status.as_u16()));
            return Err(msg);
        }

        let page_data: Vec<serde_json::Value> = resp.json().await.map_err(|e| e.to_string())?;
        let count = page_data.len();

        for repo in page_data {
            repos.push(RepoItem {
                full_name: repo["full_name"].as_str().unwrap_or("").to_string(),
                private: repo["private"].as_bool().unwrap_or(false),
                description: repo["description"].as_str().map(|s| s.to_string()),
            });
        }

        if count < 100 || page >= 10 {
            break;
        }
        page += 1;
    }

    Ok(repos)
}
