//! The OS credential store for the Navidrome password (ADR 0004): the
//! macOS Keychain, or the Secret Service (GNOME Keyring, KWallet) on Linux.
//!
//! The only module that uses `keyring-core` and its store crates. Every
//! call blocks (and may show an OS prompt), so callers run it on a
//! blocking thread. Entries live under the service [`SERVICE`] with an
//! account naming the user and server (see [`account`]).
//!
//! Errors are reduced to a [`SecretError`] kind: the store's own errors
//! can name the account (`Ambiguous` lists entries) or carry the stored
//! bytes (`BadEncoding`), so they are never displayed or logged.
//!
//! With the `mock-secret-store` feature (pty smoke runs only), and in unit
//! tests, [`SecretStore::system`] is an in-memory store instead.

use std::sync::{Arc, Mutex};

use keyring_core::{CredentialStore, Entry};
use url::Url;

use crate::navidrome::Password;

/// The service name of every entry Coda creates.
pub const SERVICE: &str = "coda";

/// What the user calls the store, for messages. A smoke-test build says
/// so, so it can never be mistaken for one that stores passwords.
#[cfg(feature = "mock-secret-store")]
pub const STORE_NAME: &str = "test store (in memory, not saved)";
#[cfg(all(not(feature = "mock-secret-store"), target_os = "macos"))]
pub const STORE_NAME: &str = "macOS Keychain";
#[cfg(all(not(feature = "mock-secret-store"), not(target_os = "macos")))]
pub const STORE_NAME: &str = "Secret Service keyring";

#[cfg(target_os = "macos")]
const UNAVAILABLE: &str = "the macOS Keychain is not available";
#[cfg(not(target_os = "macos"))]
const UNAVAILABLE: &str =
    "no Secret Service is running; install and unlock GNOME Keyring or KWallet";

/// Why a store operation failed. Never contains the account or secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    #[error("no password is stored in the {STORE_NAME}")]
    Missing,
    #[error("{UNAVAILABLE}")]
    Unavailable,
    #[error("the {STORE_NAME} is locked or denied access")]
    Denied,
    #[error("the {STORE_NAME} reported an error")]
    Failed,
}

impl SecretError {
    /// A fixed word for the log.
    pub fn kind(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Unavailable => "unavailable",
            Self::Denied => "denied",
            Self::Failed => "failed",
        }
    }

    fn from_store(error: &keyring_core::Error) -> Self {
        use keyring_core::Error;
        match error {
            Error::NoEntry => Self::Missing,
            Error::NoStorageAccess(_) => Self::Denied,
            Error::NoDefaultStore => Self::Unavailable,
            _ => Self::Failed,
        }
    }
}

/// The account of the entry for `username` on the server at `url`:
/// `username@host`, plus `:port` when it is not the scheme's default.
/// The URL path is not part of it.
pub fn account(url: &Url, username: &str) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(port) => format!("{username}@{host}:{port}"),
        None => format!("{username}@{host}"),
    }
}

/// A credential store. Cheap to clone. `Debug` names nothing.
#[derive(Clone)]
pub struct SecretStore {
    inner: Arc<CredentialStore>,
}

impl std::fmt::Debug for SecretStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretStore")
    }
}

/// The process-wide store, once it could be opened. A failure is not
/// cached, so a retry can succeed once the user has started a keyring.
static SYSTEM: Mutex<Option<SecretStore>> = Mutex::new(None);

impl SecretStore {
    /// The platform store, shared by the whole process. Blocks: on Linux
    /// this connects to the session bus.
    pub fn system() -> Result<Self, SecretError> {
        let mut system = SYSTEM.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(store) = system.as_ref() {
            return Ok(store.clone());
        }
        let store = Self::open_platform()?;
        *system = Some(store.clone());
        Ok(store)
    }

    #[cfg(any(test, feature = "mock-secret-store"))]
    fn open_platform() -> Result<Self, SecretError> {
        #[cfg(feature = "mock-secret-store")]
        tracing::warn!(
            "built with mock-secret-store: passwords go to an in-memory test store and are not saved"
        );
        Ok(Self::mock())
    }

    #[cfg(all(not(any(test, feature = "mock-secret-store")), target_os = "macos"))]
    fn open_platform() -> Result<Self, SecretError> {
        let store = apple_native_keyring_store::keychain::Store::new()
            .map_err(|_| SecretError::Unavailable)?;
        Ok(Self { inner: store })
    }

    #[cfg(all(not(any(test, feature = "mock-secret-store")), target_os = "linux"))]
    fn open_platform() -> Result<Self, SecretError> {
        let store = zbus_secret_service_keyring_store::Store::new()
            .map_err(|_| SecretError::Unavailable)?;
        Ok(Self { inner: store })
    }

    #[cfg(all(
        not(any(test, feature = "mock-secret-store")),
        not(any(target_os = "macos", target_os = "linux"))
    ))]
    fn open_platform() -> Result<Self, SecretError> {
        Err(SecretError::Unavailable)
    }

    /// A fresh, empty in-memory store (`keyring-core`'s mock store).
    #[cfg(any(test, feature = "mock-secret-store"))]
    pub fn mock() -> Self {
        let store = keyring_core::mock::Store::new().expect("the mock store cannot fail");
        Self { inner: store }
    }

    fn entry(&self, account: &str) -> Result<Entry, SecretError> {
        self.inner
            .build(SERVICE, account, None)
            .map_err(|e| SecretError::from_store(&e))
    }

    /// The stored password. An empty one counts as missing.
    pub fn get(&self, account: &str) -> Result<Password, SecretError> {
        let password = self
            .entry(account)?
            .get_password()
            .map_err(|e| SecretError::from_store(&e))?;
        if password.is_empty() {
            return Err(SecretError::Missing);
        }
        Ok(Password::new(password))
    }

    /// Stores (or replaces) the password.
    pub fn set(&self, account: &str, password: &Password) -> Result<(), SecretError> {
        self.entry(account)?
            .set_password(password.expose())
            .map_err(|e| SecretError::from_store(&e))
    }

    /// Removes the entry. Removing a missing entry is not an error.
    pub fn delete(&self, account: &str) -> Result<(), SecretError> {
        match self.entry(account)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(SecretError::from_store(&e)),
        }
    }

    /// Makes the next operation on `account` fail as `error` would.
    #[cfg(test)]
    pub(crate) fn fail_next(&self, account: &str, error: SecretError) {
        let entry = self.entry(account).unwrap();
        let cred: &keyring_core::mock::Cred = entry.as_any().downcast_ref().unwrap();
        let platform =
            || Box::new(std::io::Error::other("mock")) as keyring_core::error::PlatformError;
        cred.set_error(match error {
            SecretError::Missing => keyring_core::Error::NoEntry,
            SecretError::Denied => keyring_core::Error::NoStorageAccess(platform()),
            SecretError::Unavailable => keyring_core::Error::NoDefaultStore,
            SecretError::Failed => keyring_core::Error::PlatformFailure(platform()),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_names_user_host_and_non_default_port() {
        let url = |s: &str| Url::parse(s).unwrap();
        assert_eq!(
            account(&url("https://music.example.org/nd/"), "me"),
            "me@music.example.org"
        );
        assert_eq!(account(&url("https://h:443/"), "me"), "me@h");
        assert_eq!(account(&url("http://h:80/"), "me"), "me@h");
        assert_eq!(account(&url("http://h:4533/"), "a@b"), "a@b@h:4533");
        assert_eq!(account(&url("http://[::1]:4533/"), "me"), "me@[::1]:4533");
    }

    #[test]
    fn set_get_delete_round_trip() {
        let store = SecretStore::mock();
        assert_eq!(store.get("me@h").unwrap_err(), SecretError::Missing);
        store.set("me@h", &Password::new("s3cret".into())).unwrap();
        assert_eq!(store.get("me@h").unwrap().expose(), "s3cret");
        // Another account is another entry.
        assert_eq!(store.get("you@h").unwrap_err(), SecretError::Missing);
        store.set("me@h", &Password::new("new".into())).unwrap();
        assert_eq!(store.get("me@h").unwrap().expose(), "new");
        store.delete("me@h").unwrap();
        assert_eq!(store.get("me@h").unwrap_err(), SecretError::Missing);
        store.delete("me@h").unwrap();
    }

    #[test]
    fn store_errors_are_reduced_to_kinds() {
        let store = SecretStore::mock();
        for error in [
            SecretError::Missing,
            SecretError::Denied,
            SecretError::Unavailable,
            SecretError::Failed,
        ] {
            store.fail_next("me@h", error);
            assert_eq!(
                store.set("me@h", &Password::new("p".into())).unwrap_err(),
                error
            );
            let text = error.to_string();
            assert!(!text.contains("me@h"), "{text}");
        }
        // The failure is one-off.
        store.set("me@h", &Password::new("p".into())).unwrap();
        store.fail_next("me@h", SecretError::Denied);
        assert_eq!(store.get("me@h").unwrap_err(), SecretError::Denied);
        assert_eq!(store.get("me@h").unwrap().expose(), "p");
        assert_eq!(format!("{store:?}"), "SecretStore");
    }

    #[test]
    fn system_store_in_tests_is_a_shared_mock() {
        let a = SecretStore::system().unwrap();
        let b = SecretStore::system().unwrap();
        a.set("shared@test", &Password::new("x".into())).unwrap();
        assert_eq!(b.get("shared@test").unwrap().expose(), "x");
    }
}
