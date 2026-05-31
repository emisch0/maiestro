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

/// The context a credential is bound to — always an identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialScope {
    Identity { identity_id: String },
}

impl CredentialScope {
    fn scope_key(&self) -> String {
        match self {
            Self::Identity { identity_id } => format!("identity.{identity_id}"),
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
    CredentialStore::set(&type_id, &scope, &secret)?;
    if let CredentialScope::Identity { ref identity_id } = scope {
        crate::identities::register(identity_id);
    }
    Ok(())
}

#[tauri::command]
pub fn credentials_get(
    type_id: String,
    scope: CredentialScope,
) -> Result<String, CredentialError> {
    let value = CredentialStore::get(&type_id, &scope)?;
    if let CredentialScope::Identity { ref identity_id } = scope {
        crate::identities::register(identity_id);
    }
    Ok(value)
}

#[tauri::command]
pub fn credentials_delete(
    type_id: String,
    scope: CredentialScope,
) -> Result<(), CredentialError> {
    CredentialStore::delete(&type_id, &scope)
}
