use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("credential not found")]
    NotFound,
    #[error("keychain error: {0}")]
    Keychain(#[from] keyring::Error),
}

impl serde::Serialize for CredentialError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

/// The context a credential is bound to.
///
/// At spawn time the store tries Repo first, then falls back to Profile, so a
/// per-repo override takes precedence without removing the profile default.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialScope {
    Profile { profile_id: String },
    Repo { repo: String }, // "owner/name"
}

impl CredentialScope {
    fn scope_key(&self) -> String {
        match self {
            Self::Profile { profile_id } => format!("profile.{profile_id}"),
            Self::Repo { repo } => format!("repo.{repo}"),
        }
    }
}

pub struct CredentialStore;

impl CredentialStore {
    pub fn set(type_id: &str, scope: &CredentialScope, secret: &str) -> Result<(), CredentialError> {
        Self::entry(type_id, scope)?.set_password(secret)?;
        Ok(())
    }

    pub fn get(type_id: &str, scope: &CredentialScope) -> Result<String, CredentialError> {
        match Self::entry(type_id, scope)?.get_password() {
            Ok(secret) => Ok(secret),
            Err(keyring::Error::NoEntry) => Err(CredentialError::NotFound),
            Err(e) => Err(e.into()),
        }
    }

    pub fn delete(type_id: &str, scope: &CredentialScope) -> Result<(), CredentialError> {
        match Self::entry(type_id, scope)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Err(CredentialError::NotFound),
            Err(e) => Err(e.into()),
        }
    }

    /// Resolve a credential: tries the repo-scoped entry first, falls back to the profile.
    pub fn resolve(type_id: &str, repo: &str, profile_id: &str) -> Result<String, CredentialError> {
        let repo_scope = CredentialScope::Repo { repo: repo.to_owned() };
        match Self::get(type_id, &repo_scope) {
            Ok(v) => Ok(v),
            Err(CredentialError::NotFound) => {
                let profile_scope = CredentialScope::Profile { profile_id: profile_id.to_owned() };
                Self::get(type_id, &profile_scope)
            }
            Err(e) => Err(e),
        }
    }

    fn entry(type_id: &str, scope: &CredentialScope) -> Result<keyring::Entry, keyring::Error> {
        let service = format!("com.maiestro.cred.{}.{}", type_id, scope.scope_key());
        keyring::Entry::new(&service, "credential")
    }
}

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
pub fn credentials_set(
    type_id: String,
    scope: CredentialScope,
    secret: String,
) -> Result<(), CredentialError> {
    CredentialStore::set(&type_id, &scope, &secret)
}

#[tauri::command]
pub fn credentials_get(
    type_id: String,
    scope: CredentialScope,
) -> Result<String, CredentialError> {
    CredentialStore::get(&type_id, &scope)
}

#[tauri::command]
pub fn credentials_delete(
    type_id: String,
    scope: CredentialScope,
) -> Result<(), CredentialError> {
    CredentialStore::delete(&type_id, &scope)
}

#[tauri::command]
pub fn credentials_resolve(
    type_id: String,
    repo: String,
    profile_id: String,
) -> Result<String, CredentialError> {
    CredentialStore::resolve(&type_id, &repo, &profile_id)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    struct EntryGuard {
        type_id: String,
        scope: CredentialScope,
    }

    impl Drop for EntryGuard {
        fn drop(&mut self) {
            let _ = CredentialStore::delete(&self.type_id, &self.scope);
        }
    }

    fn unique_profile_scope() -> CredentialScope {
        CredentialScope::Profile { profile_id: Uuid::new_v4().to_string() }
    }

    #[test]
    fn set_get_delete_roundtrip() {
        let scope = unique_profile_scope();
        let _guard = EntryGuard { type_id: "github_token".into(), scope: scope.clone() };

        CredentialStore::set("github_token", &scope, "ghp_test_value").unwrap();
        assert_eq!(CredentialStore::get("github_token", &scope).unwrap(), "ghp_test_value");

        CredentialStore::delete("github_token", &scope).unwrap();
        assert!(matches!(
            CredentialStore::get("github_token", &scope),
            Err(CredentialError::NotFound)
        ));
    }

    #[test]
    fn overwrite_replaces_previous_value() {
        let scope = unique_profile_scope();
        let _guard = EntryGuard { type_id: "anthropic_key".into(), scope: scope.clone() };

        CredentialStore::set("anthropic_key", &scope, "first").unwrap();
        CredentialStore::set("anthropic_key", &scope, "second").unwrap();
        assert_eq!(CredentialStore::get("anthropic_key", &scope).unwrap(), "second");
    }

    #[test]
    fn missing_entry_returns_not_found() {
        let scope = unique_profile_scope();
        assert!(matches!(
            CredentialStore::get("github_token", &scope),
            Err(CredentialError::NotFound)
        ));
    }

    #[test]
    fn repo_scope_overrides_profile() {
        let profile_id = Uuid::new_v4().to_string();
        let repo = format!("test-org/{}", Uuid::new_v4());

        let profile_scope = CredentialScope::Profile { profile_id: profile_id.clone() };
        let repo_scope = CredentialScope::Repo { repo: repo.clone() };

        let _g1 = EntryGuard { type_id: "github_token".into(), scope: profile_scope.clone() };
        let _g2 = EntryGuard { type_id: "github_token".into(), scope: repo_scope.clone() };

        CredentialStore::set("github_token", &profile_scope, "profile_token").unwrap();
        CredentialStore::set("github_token", &repo_scope, "repo_token").unwrap();

        assert_eq!(
            CredentialStore::resolve("github_token", &repo, &profile_id).unwrap(),
            "repo_token"
        );
    }

    #[test]
    fn resolve_falls_back_to_profile() {
        let profile_id = Uuid::new_v4().to_string();
        let repo = format!("test-org/{}", Uuid::new_v4());

        let profile_scope = CredentialScope::Profile { profile_id: profile_id.clone() };
        let _guard = EntryGuard { type_id: "anthropic_key".into(), scope: profile_scope.clone() };

        CredentialStore::set("anthropic_key", &profile_scope, "sk-ant-profile").unwrap();

        assert_eq!(
            CredentialStore::resolve("anthropic_key", &repo, &profile_id).unwrap(),
            "sk-ant-profile"
        );
    }

    #[test]
    fn plugin_defined_type_id_works() {
        let scope = unique_profile_scope();
        let _guard = EntryGuard { type_id: "custom_plugin_token".into(), scope: scope.clone() };

        CredentialStore::set("custom_plugin_token", &scope, "tok_custom").unwrap();
        assert_eq!(CredentialStore::get("custom_plugin_token", &scope).unwrap(), "tok_custom");
    }
}
