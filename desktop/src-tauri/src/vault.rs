//! Secret vault abstraction (docs/INTERFACES.md §7). Secrets live only behind
//! `SecretStore`; the renderer never receives them and no IPC command returns one.
//! The OS backend is the maintained `keyring` crate with its native Windows
//! Credential Manager / macOS Keychain stores (owner-approved 19 Oct 2026). Entries
//! are addressed only by exact validated keys: nothing enumerates existing
//! credentials, and a backend failure is reported, never replaced by a plaintext
//! fallback. Runtime use of the real OS vault is not exercised by default tests.
use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

pub const SERVICE: &str = "rei-os";

#[derive(Debug, PartialEq, Eq)]
pub enum VaultError {
    InvalidKey,
    NotFound,
    /// No OS vault backend is available on this build/platform.
    Unavailable,
    Backend,
}

/// Validated vault key `rei-os/{account_id}/{kind}`. Account IDs are restricted to a
/// conservative charset so a key can never address another service's entries.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct VaultKey(String);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SecretKind {
    Refresh,
    SyncMasterKey,
    /// HIZ IMAP password (owner-entered once, Rust-owned afterwards).
    HizPassword,
    /// Microsoft Teams refresh token (rotated on every refresh).
    TeamsRefresh,
    /// Owner-supplied runtime AI key; independent of email/sync/development credentials.
    AiApiKey,
}

impl VaultKey {
    pub fn new(account_id: &str, kind: SecretKind) -> Result<Self, VaultError> {
        let ok = !account_id.is_empty()
            && account_id.len() <= 64
            && account_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !ok {
            return Err(VaultError::InvalidKey);
        }
        let kind = match kind {
            SecretKind::Refresh => "refresh",
            SecretKind::SyncMasterKey => "sync-master",
            SecretKind::HizPassword => "hiz-password",
            SecretKind::TeamsRefresh => "teams-refresh",
            SecretKind::AiApiKey => "ai-api-key",
        };
        Ok(VaultKey(format!("{SERVICE}/{account_id}/{kind}")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for VaultKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VaultKey({})", self.0)
    }
}

/// Secret bytes that never print and are zeroed on drop.
pub struct Secret(Vec<u8>);

impl Secret {
    pub fn new(bytes: Vec<u8>) -> Self {
        Secret(bytes)
    }
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        for b in self.0.iter_mut() {
            // Volatile write so the zeroing is not optimised away.
            unsafe { std::ptr::write_volatile(b, 0) };
        }
    }
}

pub trait SecretStore: Send + Sync {
    fn put(&self, key: &VaultKey, secret: &Secret) -> Result<(), VaultError>;
    fn get(&self, key: &VaultKey) -> Result<Secret, VaultError>;
    fn delete(&self, key: &VaultKey) -> Result<(), VaultError>;

    /// Atomic rotation contract: the new secret is written before the old one is
    /// discarded, so a crash never leaves the account without a refresh token.
    fn rotate(&self, key: &VaultKey, new: &Secret) -> Result<(), VaultError> {
        self.put(key, new)
    }
}

/// Process-memory store. Used for tests and for memory-only access tokens; it is
/// never persisted and is not a substitute for the OS vault for refresh tokens.
#[derive(Default)]
pub struct MemoryStore {
    map: Mutex<HashMap<VaultKey, Vec<u8>>>,
}

impl SecretStore for MemoryStore {
    fn put(&self, key: &VaultKey, secret: &Secret) -> Result<(), VaultError> {
        let mut m = self.map.lock().map_err(|_| VaultError::Backend)?;
        m.insert(key.clone(), secret.expose().to_vec());
        Ok(())
    }
    fn get(&self, key: &VaultKey) -> Result<Secret, VaultError> {
        let m = self.map.lock().map_err(|_| VaultError::Backend)?;
        m.get(key)
            .map(|v| Secret::new(v.clone()))
            .ok_or(VaultError::NotFound)
    }
    fn delete(&self, key: &VaultKey) -> Result<(), VaultError> {
        let mut m = self.map.lock().map_err(|_| VaultError::Backend)?;
        if let Some(mut v) = m.remove(key) {
            v.iter_mut().for_each(|b| *b = 0);
        }
        Ok(())
    }
}

/// OS vault via `keyring` native backends. Service = `rei-os`, user = full key.
pub struct KeyringStore;

fn keyring_entry(key: &VaultKey) -> Result<keyring::Entry, VaultError> {
    keyring::Entry::new(SERVICE, key.as_str()).map_err(map_keyring)
}

/// Maps backend errors without carrying their text (it may echo attributes).
fn map_keyring(e: keyring::Error) -> VaultError {
    match e {
        keyring::Error::NoEntry => VaultError::NotFound,
        keyring::Error::NoStorageAccess(_) | keyring::Error::PlatformFailure(_) => {
            VaultError::Unavailable
        }
        _ => VaultError::Backend,
    }
}

impl SecretStore for KeyringStore {
    fn put(&self, key: &VaultKey, secret: &Secret) -> Result<(), VaultError> {
        keyring_entry(key)?
            .set_secret(secret.expose())
            .map_err(map_keyring)
    }
    fn get(&self, key: &VaultKey) -> Result<Secret, VaultError> {
        keyring_entry(key)?
            .get_secret()
            .map(Secret::new)
            .map_err(map_keyring)
    }
    fn delete(&self, key: &VaultKey) -> Result<(), VaultError> {
        match keyring_entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(map_keyring(e)),
        }
    }
}

/// OS vault backend on Windows and macOS; other platforms fail closed.
pub fn platform_store() -> Result<Box<dyn SecretStore>, VaultError> {
    if cfg!(any(target_os = "windows", target_os = "macos")) {
        Ok(Box::new(KeyringStore))
    } else {
        Err(VaultError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_rejects_traversal_and_foreign_namespaces() {
        for bad in ["", "../x", "a/b", "a b", "ä", &"x".repeat(65)] {
            assert_eq!(
                VaultKey::new(bad, SecretKind::Refresh),
                Err(VaultError::InvalidKey)
            );
        }
        let k = VaultKey::new("acct-1", SecretKind::Refresh).unwrap();
        assert_eq!(k.as_str(), "rei-os/acct-1/refresh");
    }

    #[test]
    fn secret_debug_is_redacted() {
        let s = Secret::new(b"super-refresh-token".to_vec());
        assert_eq!(format!("{s:?}"), "Secret(<redacted>)");
    }

    #[test]
    fn provider_secret_kinds_have_separate_keys() {
        let p = VaultKey::new("acct-1", SecretKind::HizPassword).unwrap();
        let t = VaultKey::new("acct-1", SecretKind::TeamsRefresh).unwrap();
        assert_eq!(p.as_str(), "rei-os/acct-1/hiz-password");
        assert_eq!(t.as_str(), "rei-os/acct-1/teams-refresh");
        // Constructing the backend never touches the OS vault (no read, no enumeration).
        assert_eq!(
            platform_store().is_ok(),
            cfg!(any(target_os = "windows", target_os = "macos"))
        );
    }
}
