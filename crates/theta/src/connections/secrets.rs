//! Connection secrets: OS keychain when available, memory otherwise.
//!
//! Passwords never touch `connections.json`. They are stored as one JSON blob
//! per connection id in the platform keychain (service "theta"; the former "quarry" service is read as a fallback). If the
//! keychain cannot be used (no Secret Service on a headless machine, a locked
//! keyring, a sandbox) the secrets of the running session are kept in memory so
//! connecting still works, and [`SecretStore::persistent`] reports `false` so the
//! connection dialog can say "Password will not be saved (no system keychain)".

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use theta_core::{ConnectionConfig, SshAuth};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Secret {
    pub password: Option<String>,
    pub ssh_password: Option<String>,
    pub ssh_passphrase: Option<String>,
}

impl Secret {
    pub fn is_empty(&self) -> bool {
        self.password.is_none() && self.ssh_password.is_none() && self.ssh_passphrase.is_none()
    }

    pub fn from_config(config: &ConnectionConfig) -> Secret {
        let mut s =
            Secret { password: config.password.clone().filter(|p| !p.is_empty()), ..Default::default() };
        match config.ssh.as_ref().map(|s| &s.auth) {
            Some(SshAuth::Password { password }) if !password.is_empty() => {
                s.ssh_password = Some(password.clone())
            }
            Some(SshAuth::KeyFile { passphrase: Some(p), .. }) if !p.is_empty() => {
                s.ssh_passphrase = Some(p.clone())
            }
            _ => {}
        }
        s
    }

    pub fn apply_to(&self, config: &mut ConnectionConfig) {
        if self.password.is_some() {
            config.password = self.password.clone();
        }
        if let Some(ssh) = config.ssh.as_mut() {
            match &mut ssh.auth {
                SshAuth::Password { password } => {
                    if let Some(p) = &self.ssh_password {
                        *password = p.clone();
                    }
                }
                SshAuth::KeyFile { passphrase, .. } => {
                    if self.ssh_passphrase.is_some() {
                        *passphrase = self.ssh_passphrase.clone();
                    }
                }
                SshAuth::Agent => {}
            }
        }
    }
}

/// Remove every secret so the config can be written to disk.
pub fn strip_secrets(config: &ConnectionConfig) -> ConnectionConfig {
    let mut c = config.clone();
    c.password = None;
    if let Some(ssh) = c.ssh.as_mut() {
        match &mut ssh.auth {
            SshAuth::Password { password } => password.clear(),
            SshAuth::KeyFile { passphrase, .. } => *passphrase = None,
            SshAuth::Agent => {}
        }
    }
    c
}

/// Where secrets are persisted. Implemented by the OS keychain and by test fakes.
pub trait SecretBackend: Send + Sync {
    fn set(&self, id: &str, value: &str) -> Result<(), String>;
    fn get(&self, id: &str) -> Result<Option<String>, String>;
    fn delete(&self, id: &str) -> Result<(), String>;
}

pub struct KeyringBackend {
    service: &'static str,
}

const SERVICE: &str = "theta";
/// The keychain service name before the rename from Quarry.
const LEGACY_SERVICE: &str = "quarry";

impl KeyringBackend {
    /// Reads fall back to the pre-rename service so saved passwords survive the upgrade.
    pub fn system() -> impl SecretBackend {
        WithLegacy {
            current: KeyringBackend { service: SERVICE },
            legacy: KeyringBackend { service: LEGACY_SERVICE },
        }
    }
}

impl SecretBackend for KeyringBackend {
    fn set(&self, id: &str, value: &str) -> Result<(), String> {
        keyring::Entry::new(self.service, id).and_then(|e| e.set_password(value)).map_err(|e| e.to_string())
    }
    fn get(&self, id: &str) -> Result<Option<String>, String> {
        match keyring::Entry::new(self.service, id).and_then(|e| e.get_password()) {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        match keyring::Entry::new(self.service, id).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Writes go to `current`; a read that misses there is answered from `legacy` and copied
/// forward, so each secret is migrated the first time it is used. The legacy entry is left
/// in place: deleting a user's only copy of a password is not worth the tidiness, and
/// `delete` removes both so a removed connection leaves nothing behind.
struct WithLegacy<B> {
    current: B,
    legacy: B,
}

impl<B: SecretBackend> SecretBackend for WithLegacy<B> {
    fn set(&self, id: &str, value: &str) -> Result<(), String> {
        self.current.set(id, value)
    }
    fn get(&self, id: &str) -> Result<Option<String>, String> {
        if let Some(v) = self.current.get(id)? {
            return Ok(Some(v));
        }
        // A failing legacy lookup must not hide the (successful) "no entry" answer above.
        let Some(v) = self.legacy.get(id).ok().flatten() else { return Ok(None) };
        let _ = self.current.set(id, &v);
        Ok(Some(v))
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        let _ = self.legacy.delete(id);
        self.current.delete(id)
    }
}

pub struct SecretStore {
    backend: Option<Box<dyn SecretBackend>>,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    memory: HashMap<String, Secret>,
    /// Set after the first backend failure; the keychain is not retried every call
    /// because a failing D-Bus lookup can take noticeable time.
    backend_failed: Option<String>,
    probed: bool,
}

impl SecretStore {
    pub fn new(backend: Option<Box<dyn SecretBackend>>) -> Self {
        Self { backend, inner: Mutex::new(Inner::default()) }
    }

    pub fn system() -> Self {
        Self::new(Some(Box::new(KeyringBackend::system())))
    }

    /// Memory only (used by tests and when the keychain is known to be absent).
    pub fn memory_only() -> Self {
        Self::new(None)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether secrets survive an app restart. Probes the keychain once.
    pub fn persistent(&self) -> bool {
        let Some(backend) = &self.backend else { return false };
        {
            let inner = self.lock();
            if inner.backend_failed.is_some() {
                return false;
            }
            if inner.probed {
                return true;
            }
        }
        let probe_id = "__theta_probe__";
        let result = backend.set(probe_id, "ok").and_then(|_| backend.get(probe_id)).and_then(|v| {
            if v.as_deref() == Some("ok") { Ok(()) } else { Err("probe value mismatch".to_string()) }
        });
        let _ = backend.delete(probe_id);
        let mut inner = self.lock();
        inner.probed = true;
        if let Err(e) = result {
            inner.backend_failed = Some(e);
            return false;
        }
        true
    }

    pub fn backend_error(&self) -> Option<String> {
        self.lock().backend_failed.clone()
    }

    fn backend_usable(&self) -> Option<&dyn SecretBackend> {
        if self.lock().backend_failed.is_some() {
            return None;
        }
        self.backend.as_deref()
    }

    fn note_failure(&self, e: String) {
        self.lock().backend_failed = Some(e);
    }

    pub fn save(&self, id: &str, secret: &Secret) {
        if secret.is_empty() {
            self.remove(id);
            return;
        }
        self.lock().memory.insert(id.to_string(), secret.clone());
        if let Some(b) = self.backend_usable()
            && let Err(e) = b.set(id, &serde_json::to_string(secret).unwrap_or_default())
        {
            self.note_failure(e);
        }
    }

    pub fn load(&self, id: &str) -> Secret {
        if let Some(s) = self.lock().memory.get(id) {
            return s.clone();
        }
        if let Some(b) = self.backend_usable() {
            match b.get(id) {
                Ok(Some(text)) => {
                    let s: Secret = serde_json::from_str(&text).unwrap_or_default();
                    self.lock().memory.insert(id.to_string(), s.clone());
                    return s;
                }
                Ok(None) => {}
                Err(e) => self.note_failure(e),
            }
        }
        Secret::default()
    }

    pub fn remove(&self, id: &str) {
        self.lock().memory.remove(id);
        if let Some(b) = self.backend_usable()
            && let Err(e) = b.delete(id)
        {
            self.note_failure(e);
        }
    }

    /// `config` with its secrets filled in from the store.
    pub fn with_secrets(&self, config: &ConnectionConfig) -> ConnectionConfig {
        let mut c = config.clone();
        self.load(&config.id).apply_to(&mut c);
        c
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;

    #[derive(Default)]
    pub struct FakeBackend {
        pub map: Mutex<HashMap<String, String>>,
        pub fail: bool,
    }

    impl SecretBackend for std::sync::Arc<FakeBackend> {
        fn set(&self, id: &str, value: &str) -> Result<(), String> {
            if self.fail {
                return Err("no keychain".into());
            }
            self.map.lock().unwrap().insert(id.into(), value.into());
            Ok(())
        }
        fn get(&self, id: &str) -> Result<Option<String>, String> {
            if self.fail {
                return Err("no keychain".into());
            }
            Ok(self.map.lock().unwrap().get(id).cloned())
        }
        fn delete(&self, id: &str) -> Result<(), String> {
            if self.fail {
                return Err("no keychain".into());
            }
            self.map.lock().unwrap().remove(id);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeBackend;
    use super::*;
    use std::sync::Arc;
    use theta_core::{DatabaseKind, SshConfig};

    fn config_with_secrets() -> ConnectionConfig {
        let mut c = ConnectionConfig::new("c1", "prod", DatabaseKind::Postgres);
        c.password = Some("hunter2".into());
        c.ssh = Some(SshConfig {
            host: "jump".into(),
            port: 22,
            user: "me".into(),
            auth: SshAuth::KeyFile { path: "/k".into(), passphrase: Some("phrase".into()) },
        });
        c
    }

    #[test]
    fn legacy_service_secrets_are_read_and_copied_forward() {
        let (current, legacy) = (Arc::new(FakeBackend::default()), Arc::new(FakeBackend::default()));
        legacy.set("c1", "old-secret").unwrap();
        let b = WithLegacy { current: current.clone(), legacy: legacy.clone() };
        assert_eq!(b.get("c1").unwrap().as_deref(), Some("old-secret"));
        assert_eq!(current.get("c1").unwrap().as_deref(), Some("old-secret"));
        assert_eq!(b.get("missing").unwrap(), None);
        // A value in the current service always wins over a stale legacy one.
        current.set("c1", "new-secret").unwrap();
        assert_eq!(b.get("c1").unwrap().as_deref(), Some("new-secret"));
        // Deleting a connection's secret removes both copies.
        b.delete("c1").unwrap();
        assert_eq!((current.get("c1").unwrap(), legacy.get("c1").unwrap()), (None, None));
    }

    #[test]
    fn strip_removes_every_secret() {
        let stripped = strip_secrets(&config_with_secrets());
        assert_eq!(stripped.password, None);
        match stripped.ssh.unwrap().auth {
            SshAuth::KeyFile { path, passphrase } => assert_eq!((path.as_str(), passphrase), ("/k", None)),
            _ => panic!(),
        }
        let mut c = config_with_secrets();
        c.ssh.as_mut().unwrap().auth = SshAuth::Password { password: "pw".into() };
        match strip_secrets(&c).ssh.unwrap().auth {
            SshAuth::Password { password } => assert!(password.is_empty()),
            _ => panic!(),
        }
        let json = serde_json::to_string(&strip_secrets(&config_with_secrets())).unwrap();
        assert!(!json.contains("hunter2") && !json.contains("\"phrase\""));
    }

    #[test]
    fn secret_round_trips_through_config() {
        let cfg = config_with_secrets();
        let secret = Secret::from_config(&cfg);
        assert_eq!(secret.password.as_deref(), Some("hunter2"));
        assert_eq!(secret.ssh_passphrase.as_deref(), Some("phrase"));
        let mut restored = strip_secrets(&cfg);
        secret.apply_to(&mut restored);
        assert_eq!(restored, cfg);
    }

    #[test]
    fn keychain_backend_is_used_when_available() {
        let fake = Arc::new(FakeBackend::default());
        let store = SecretStore::new(Some(Box::new(fake.clone())));
        assert!(store.persistent());
        store.save("c1", &Secret { password: Some("pw".into()), ..Default::default() });
        assert!(fake.map.lock().unwrap().contains_key("c1"));
        // A second store (an app restart) reads it back from the keychain.
        let again = SecretStore::new(Some(Box::new(fake.clone())));
        assert_eq!(again.load("c1").password.as_deref(), Some("pw"));
        again.remove("c1");
        assert!(fake.map.lock().unwrap().is_empty());
    }

    #[test]
    fn failing_keychain_falls_back_to_memory() {
        let fake = Arc::new(FakeBackend { fail: true, ..Default::default() });
        let store = SecretStore::new(Some(Box::new(fake)));
        assert!(!store.persistent());
        assert!(store.backend_error().is_some());
        store.save("c1", &Secret { password: Some("pw".into()), ..Default::default() });
        assert_eq!(store.load("c1").password.as_deref(), Some("pw"), "session memory still works");
        let cfg = ConnectionConfig::new("c1", "x", DatabaseKind::MySql);
        assert_eq!(store.with_secrets(&cfg).password.as_deref(), Some("pw"));
    }

    #[test]
    fn memory_only_is_not_persistent() {
        assert!(!SecretStore::memory_only().persistent());
    }

    #[test]
    fn saving_an_empty_secret_removes_it() {
        let store = SecretStore::memory_only();
        store.save("c1", &Secret { password: Some("pw".into()), ..Default::default() });
        store.save("c1", &Secret::default());
        assert!(store.load("c1").is_empty());
    }
}
