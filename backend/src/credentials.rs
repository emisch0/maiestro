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

// The public API delegates to a backend chosen at compile time: the real macOS
// Keychain in normal builds, an in-memory map under `cfg(test)`. Splitting here
// (rather than a runtime trait object) keeps the shipped code path identical to
// before while letting every test that resolves a credential — identity → token,
// the GitHub client constructor, the health check — run with no Keychain access
// (which would otherwise prompt, block, or write to the developer's real login
// keychain). See the `cfg(test)` in-memory `backend` module below.
impl CredentialStore {
    pub fn set(type_id: &str, scope: &CredentialScope, secret: &str) -> Result<(), CredentialError> {
        backend::set(type_id, scope, secret)
    }

    pub fn get(type_id: &str, scope: &CredentialScope) -> Result<String, CredentialError> {
        backend::get(type_id, scope)
    }

    pub fn delete(type_id: &str, scope: &CredentialScope) -> Result<(), CredentialError> {
        backend::delete(type_id, scope)
    }
}

/// The real Keychain backend, used in every non-test build. Byte-for-byte the
/// behavior the app has always had — the service name and error mapping are
/// unchanged; only the enclosing function moved.
#[cfg(not(test))]
mod backend {
    use super::{CredentialError, CredentialScope};

    fn entry(type_id: &str, scope: &CredentialScope) -> Result<keyring::Entry, keyring::Error> {
        let service = format!("com.maiestro.cred.{}.{}", type_id, scope.scope_key());
        keyring::Entry::new(&service, "credential")
    }

    pub fn set(type_id: &str, scope: &CredentialScope, secret: &str) -> Result<(), CredentialError> {
        entry(type_id, scope)?.set_password(secret)?;
        Ok(())
    }

    pub fn get(type_id: &str, scope: &CredentialScope) -> Result<String, CredentialError> {
        match entry(type_id, scope)?.get_password() {
            Ok(secret) => Ok(secret),
            Err(keyring::Error::NoEntry) => Err(CredentialError::NotFound),
            Err(e) => Err(e.into()),
        }
    }

    pub fn delete(type_id: &str, scope: &CredentialScope) -> Result<(), CredentialError> {
        match entry(type_id, scope)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Err(CredentialError::NotFound),
            Err(e) => Err(e.into()),
        }
    }
}

/// In-memory backend for `cfg(test)`: a process-global map keyed exactly like the
/// Keychain service name, so tests exercise the same `type_id`/scope-keying logic
/// without touching the OS. Tests should use unique identity ids (or call
/// [`test_backend::clear`]) since the map is shared across the test binary.
#[cfg(test)]
mod backend {
    use super::{CredentialError, CredentialScope};
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    fn store() -> &'static Mutex<HashMap<String, String>> {
        static STORE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
        STORE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    // Same shape as the Keychain service name, so keying bugs surface in tests too.
    fn key(type_id: &str, scope: &CredentialScope) -> String {
        format!("com.maiestro.cred.{}.{}", type_id, scope.scope_key())
    }

    pub fn set(type_id: &str, scope: &CredentialScope, secret: &str) -> Result<(), CredentialError> {
        store().lock().unwrap().insert(key(type_id, scope), secret.to_string());
        Ok(())
    }

    pub fn get(type_id: &str, scope: &CredentialScope) -> Result<String, CredentialError> {
        store()
            .lock()
            .unwrap()
            .get(&key(type_id, scope))
            .cloned()
            .ok_or(CredentialError::NotFound)
    }

    pub fn delete(type_id: &str, scope: &CredentialScope) -> Result<(), CredentialError> {
        match store().lock().unwrap().remove(&key(type_id, scope)) {
            Some(_) => Ok(()),
            None => Err(CredentialError::NotFound),
        }
    }
}

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
pub fn credentials_set(
    type_id: String,
    scope: CredentialScope,
    secret: String,
) -> Result<(), CredentialError> {
    let CredentialScope::Identity { identity_id } = &scope;
    // Log the credential type + identity scope only — never the secret value.
    crate::log_invoke!("credentials_set", type_id = %type_id, identity = %identity_id);
    CredentialStore::set(&type_id, &scope, &secret)?;
    crate::identities::register(identity_id);
    Ok(())
}

/// Whether a credential is stored for this type + scope. Returns only a boolean —
/// the secret value is never handed to the webview (the frontend only needs "is it
/// set?"). The backend reads the token itself via `GitHub::for_identity` when it
/// actually needs it, so no command exposes the raw value across the IPC boundary.
#[tauri::command]
pub fn credentials_exists(
    type_id: String,
    scope: CredentialScope,
) -> Result<bool, CredentialError> {
    let CredentialScope::Identity { identity_id } = &scope;
    // Log the credential type + identity scope only — never the secret value.
    crate::log_invoke_debug!("credentials_exists", type_id = %type_id, identity = %identity_id);
    match CredentialStore::get(&type_id, &scope) {
        Ok(_) => Ok(true),
        Err(CredentialError::NotFound) => Ok(false),
        Err(e) => Err(e),
    }
}

#[tauri::command]
pub fn credentials_delete(
    type_id: String,
    scope: CredentialScope,
) -> Result<(), CredentialError> {
    let CredentialScope::Identity { identity_id } = &scope;
    crate::log_invoke!("credentials_delete", type_id = %type_id, identity = %identity_id);
    CredentialStore::delete(&type_id, &scope)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(id: &str) -> CredentialScope {
        CredentialScope::Identity { identity_id: id.to_string() }
    }

    #[test]
    fn scope_key_is_stable() {
        assert_eq!(scope("work").scope_key(), "identity.work");
    }

    #[test]
    fn set_get_delete_roundtrip() {
        // Unique id so the process-global test store can't collide with siblings.
        let s = scope("cred-roundtrip");

        // Missing → NotFound.
        assert!(matches!(
            CredentialStore::get("github_token", &s),
            Err(CredentialError::NotFound)
        ));

        // Set then get returns the secret.
        CredentialStore::set("github_token", &s, "ghp_secret").unwrap();
        assert_eq!(CredentialStore::get("github_token", &s).unwrap(), "ghp_secret");

        // Overwrite replaces it.
        CredentialStore::set("github_token", &s, "ghp_rotated").unwrap();
        assert_eq!(CredentialStore::get("github_token", &s).unwrap(), "ghp_rotated");

        // Delete, then it's gone; deleting again is NotFound.
        CredentialStore::delete("github_token", &s).unwrap();
        assert!(matches!(
            CredentialStore::get("github_token", &s),
            Err(CredentialError::NotFound)
        ));
        assert!(matches!(
            CredentialStore::delete("github_token", &s),
            Err(CredentialError::NotFound)
        ));
    }

    #[test]
    fn distinct_identities_do_not_share_secrets() {
        CredentialStore::set("github_token", &scope("cred-a"), "tok-a").unwrap();
        CredentialStore::set("github_token", &scope("cred-b"), "tok-b").unwrap();
        assert_eq!(CredentialStore::get("github_token", &scope("cred-a")).unwrap(), "tok-a");
        assert_eq!(CredentialStore::get("github_token", &scope("cred-b")).unwrap(), "tok-b");
    }
}
