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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    GithubToken,
    AnthropicKey,
}

impl CredentialKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::GithubToken => "github_token",
            Self::AnthropicKey => "anthropic_key",
        }
    }
}

pub struct CredentialStore;

impl CredentialStore {
    pub fn set(profile_id: &str, kind: CredentialKind, secret: &str) -> Result<(), CredentialError> {
        Self::entry(profile_id, kind)?.set_password(secret)?;
        Ok(())
    }

    pub fn get(profile_id: &str, kind: CredentialKind) -> Result<String, CredentialError> {
        match Self::entry(profile_id, kind)?.get_password() {
            Ok(secret) => Ok(secret),
            Err(keyring::Error::NoEntry) => Err(CredentialError::NotFound),
            Err(e) => Err(e.into()),
        }
    }

    pub fn delete(profile_id: &str, kind: CredentialKind) -> Result<(), CredentialError> {
        match Self::entry(profile_id, kind)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Err(CredentialError::NotFound),
            Err(e) => Err(e.into()),
        }
    }

    fn entry(profile_id: &str, kind: CredentialKind) -> Result<keyring::Entry, keyring::Error> {
        // Service per the convention in issue #2:
        //   com.maiestro.agent.<profile-id>.<credential-kind>
        // Account is fixed because the service name already uniquely identifies the entry.
        let service = format!("com.maiestro.agent.{}.{}", profile_id, kind.as_str());
        keyring::Entry::new(&service, "credential")
    }
}

#[tauri::command]
pub fn credentials_set(
    profile_id: String,
    kind: CredentialKind,
    secret: String,
) -> Result<(), CredentialError> {
    CredentialStore::set(&profile_id, kind, &secret)
}

#[tauri::command]
pub fn credentials_get(profile_id: String, kind: CredentialKind) -> Result<String, CredentialError> {
    CredentialStore::get(&profile_id, kind)
}

#[tauri::command]
pub fn credentials_delete(
    profile_id: String,
    kind: CredentialKind,
) -> Result<(), CredentialError> {
    CredentialStore::delete(&profile_id, kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    /// RAII guard so a Keychain entry is purged even if the test panics.
    struct EntryGuard {
        profile_id: String,
        kind: CredentialKind,
    }

    impl Drop for EntryGuard {
        fn drop(&mut self) {
            let _ = CredentialStore::delete(&self.profile_id, self.kind);
        }
    }

    #[test]
    fn set_get_delete_roundtrip() {
        let profile_id = Uuid::new_v4().to_string();
        let _guard = EntryGuard {
            profile_id: profile_id.clone(),
            kind: CredentialKind::GithubToken,
        };

        CredentialStore::set(&profile_id, CredentialKind::GithubToken, "ghp_test_value").unwrap();
        assert_eq!(
            CredentialStore::get(&profile_id, CredentialKind::GithubToken).unwrap(),
            "ghp_test_value"
        );

        CredentialStore::delete(&profile_id, CredentialKind::GithubToken).unwrap();
        assert!(matches!(
            CredentialStore::get(&profile_id, CredentialKind::GithubToken),
            Err(CredentialError::NotFound)
        ));
    }

    #[test]
    fn overwrite_replaces_previous_value() {
        let profile_id = Uuid::new_v4().to_string();
        let _guard = EntryGuard {
            profile_id: profile_id.clone(),
            kind: CredentialKind::AnthropicKey,
        };

        CredentialStore::set(&profile_id, CredentialKind::AnthropicKey, "first").unwrap();
        CredentialStore::set(&profile_id, CredentialKind::AnthropicKey, "second").unwrap();
        assert_eq!(
            CredentialStore::get(&profile_id, CredentialKind::AnthropicKey).unwrap(),
            "second"
        );
    }

    #[test]
    fn missing_entry_returns_not_found() {
        let profile_id = Uuid::new_v4().to_string();
        assert!(matches!(
            CredentialStore::get(&profile_id, CredentialKind::GithubToken),
            Err(CredentialError::NotFound)
        ));
    }
}
