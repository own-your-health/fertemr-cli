//! Where a profile's portal session is kept: the one seam that knows.
//!
//! A [`Session`] is the portal's two ids (`duuid`, the device id fertemr
//! made for the profile, and `suuid`, the session the sign-in answered), the
//! API base they belong to, the portal user and patient ids the sign-in
//! answered, and when it was made. It is one JSON entry per profile; the
//! portal password is never kept.
//!
//! The entries are the vault's (`pk_cli_auth::vault`, cli-common DESIGN.md
//! §2.5): `<profile>/<org>/<patient>` ([`Key`]), each in its own encrypted owner-only file
//! under `~/.config/fertemr/vault/`, all under one key, the keychain item
//! `vault-key` in `piekstra.fertemr`. A run with nothing stored reads no
//! keychain item at all.
//!
//! [`KEYCHAIN_SERVICE_ENV`] names another keychain service for the key, for
//! the tests and the smoke run, and moves the vault under
//! `$XDG_CONFIG_HOME/fertemr/vault`, which must then be set: a test's vault
//! is never the owner's. While it is set ([`hermetic`]) nothing touches the
//! real browser profiles either.

use std::path::PathBuf;

use pk_cli_auth::vault::{self, Vault};
use pk_cli_config::profile::ProfileName;
use pk_cli_core::{CliError, Reason};
use pk_cli_secrets::{CredentialStore, Secret};
use serde::{Deserialize, Serialize};

use crate::cli::BIN;
use crate::portal::Device;

/// Names the keychain service holding the vault's key instead of
/// `piekstra.fertemr`, and moves the vault under `$XDG_CONFIG_HOME`. For the
/// tests and the smoke run only.
pub const KEYCHAIN_SERVICE_ENV: &str = "FERTEMR_KEYCHAIN_SERVICE";

/// The stored session. `suuid` is the secret; the rest is what `auth
/// status` reports and what a read sends beside it.
pub struct Session {
    /// The portal's web address (`https://portal-<clinic>.fertemr.com`).
    pub portal: String,
    /// The API base the sign-in went to, from the portal's config.json.
    pub api_base: String,
    pub device_id: String,
    pub session_id: Secret,
    /// The portal's ids for the signed-in user and their patient record.
    pub user_id: Option<i64>,
    pub patient_id: Option<i64>,
    /// The name the portal shows, for the stream's Patient.
    pub full_name: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    /// RFC 3339, UTC.
    pub signed_in_at: String,
}

/// A session's vault entry: `<profile>/<org>/<patient>`, the organization
/// being the portal's short name and the patient the household's label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub profile: ProfileName,
    pub org: String,
    pub patient: String,
}

impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}/{}", self.profile, self.org, self.patient)
    }
}

#[derive(Serialize, Deserialize)]
struct Entry {
    portal: String,
    api_base: String,
    device_id: String,
    session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    user_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    patient_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    full_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    first_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_name: Option<String>,
    signed_in_at: String,
}

impl Session {
    /// What a read sends.
    pub fn device(&self) -> Device {
        Device {
            duuid: self.device_id.clone(),
            suuid: Secret::new(self.session_id.expose()),
        }
    }

    fn entry(&self) -> Entry {
        Entry {
            portal: self.portal.clone(),
            api_base: self.api_base.clone(),
            device_id: self.device_id.clone(),
            session_id: self.session_id.expose().to_string(),
            user_id: self.user_id,
            patient_id: self.patient_id,
            full_name: self.full_name.clone(),
            first_name: self.first_name.clone(),
            last_name: self.last_name.clone(),
            signed_in_at: self.signed_in_at.clone(),
        }
    }

    /// The session an entry holds; `None` for one with an empty id.
    fn from_entry(e: Entry) -> Option<Session> {
        (!e.session_id.is_empty() && !e.device_id.is_empty() && !e.api_base.is_empty()).then(|| Session {
            portal: e.portal,
            api_base: e.api_base,
            device_id: e.device_id,
            session_id: Secret::new(e.session_id),
            user_id: e.user_id,
            patient_id: e.patient_id,
            full_name: e.full_name,
            first_name: e.first_name,
            last_name: e.last_name,
            signed_in_at: e.signed_in_at,
        })
    }
}

/// The sessions, one per profile.
pub struct SessionStore {
    vault: Vault,
}

impl SessionStore {
    /// The store every command uses: the vault of `piekstra.fertemr`, or with
    /// [`KEYCHAIN_SERVICE_ENV`] a test's ([`hermetic`]).
    pub fn open() -> Result<SessionStore, CliError> {
        let vault = match service_override(service_env().as_deref()) {
            None => Vault::for_binary(BIN)?,
            Some(service) => Vault::new(CredentialStore::new(service), test_vault_dir()?),
        };
        Ok(SessionStore { vault })
    }

    /// A store over `vault`: the tests' in-memory one.
    pub fn with_vault(vault: Vault) -> SessionStore {
        SessionStore { vault }
    }

    /// The session stored under `key`, `None` when none is. An entry this
    /// fertemr cannot read is an error naming the profile and the command
    /// that removes it, never what it holds.
    pub fn get(&self, key: &Key) -> Result<Option<Session>, CliError> {
        let profile = &key.profile;
        let Some(raw) = self.vault.get(&key.to_string()).map_err(|e| remedied(e, profile))? else {
            return Ok(None);
        };
        let read: Entry = serde_json::from_str(raw.expose()).map_err(|_| unreadable(profile))?;
        Session::from_entry(read).map(Some).ok_or_else(|| unreadable(profile))
    }

    /// The session, for a command that reads the portal: none stored is
    /// exit 3, with the sign-in that stores one.
    pub fn load(&self, key: &Key) -> Result<Session, CliError> {
        self.get(key)?.ok_or_else(|| not_signed_in(&key.profile))
    }

    pub fn put(&self, key: &Key, session: &Session) -> Result<(), CliError> {
        self.vault
            .put_json(&key.to_string(), &session.entry())
            .map_err(|e| remedied(e, &key.profile))
    }

    /// Whether there was a session to remove. Reads no keychain item.
    pub fn delete(&self, key: &Key) -> Result<bool, CliError> {
        self.vault.delete(&key.to_string())
    }

    /// Remove every profile's session and the vault's key (`auth reset`).
    /// How many entries were removed.
    pub fn clear(&self) -> Result<usize, CliError> {
        self.vault.clear()
    }
}

/// A profile with no session: exit 3, naming the sign-in.
pub fn not_signed_in(profile: &ProfileName) -> CliError {
    CliError::Auth(format!("no portal session is stored for profile `{profile}`"))
        .because(Reason::NoCredential)
        .with_remedy(format!(
            "{BIN} auth login --profile {profile} --portal <PORTAL> --stdin (or --op-item, or --browser)"
        ))
}

/// The vault's errors with fertemr's own commands as their remedies.
fn remedied(e: CliError, profile: &ProfileName) -> CliError {
    match e.remedy() {
        Some(vault::REMEDY_DELETE_ENTRY) => e
            .with_remedy(format!("{BIN} auth logout --profile {profile}, then sign in again"))
            .because(Reason::CredentialUnreadable),
        Some(vault::REMEDY_CLEAR) => e.with_remedy(format!(
            "{BIN} auth reset removes every profile's session and the key; then sign in again"
        )),
        _ => e,
    }
}

fn unreadable(profile: &ProfileName) -> CliError {
    CliError::Keychain(format!(
        "the session stored for profile `{profile}` is not the shape this fertemr writes"
    ))
    .because(Reason::CredentialUnreadable)
    .with_remedy(format!("{BIN} auth logout --profile {profile}, then sign in again"))
}

/// Whether [`KEYCHAIN_SERVICE_ENV`] points the store elsewhere: a test or
/// the smoke run, under which nothing reads or removes the owner's kept
/// browser profiles.
pub fn hermetic() -> bool {
    service_override(service_env().as_deref()).is_some()
}

/// A test's vault: under `$XDG_CONFIG_HOME/fertemr`, which must be set, so a
/// test never opens the owner's `~/.config/fertemr/vault`.
fn test_vault_dir() -> Result<PathBuf, CliError> {
    match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(base) => Ok(PathBuf::from(base).join(BIN).join("vault")),
        None => Err(CliError::Usage(format!(
            "{KEYCHAIN_SERVICE_ENV} is for tests and goes with XDG_CONFIG_HOME, so a test's vault is never the \
             owner's"
        ))),
    }
}

/// The variable's value. One that is not UTF-8 still counts as set, so it
/// never falls back to the real service.
fn service_env() -> Option<String> {
    std::env::var_os(KEYCHAIN_SERVICE_ENV).map(|v| v.to_string_lossy().into_owned())
}

/// A blank value names no service.
fn service_override(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pk_cli_auth::vault::MemoryKeys;
    use std::sync::Arc;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fertemr-session-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn key(profile: &str) -> Key {
        Key {
            profile: ProfileName::parse(profile).unwrap(),
            org: "someclinic".into(),
            patient: profile.into(),
        }
    }

    fn dummy() -> Session {
        Session {
            portal: "https://portal-someclinic.fertemr.com".into(),
            api_base: "https://portal-someclinic-api.fertemr.com".into(),
            device_id: "00000000-0000-4000-8000-000000000001".into(),
            session_id: Secret::new("dummy-session-0001"),
            user_id: Some(1),
            patient_id: Some(2),
            full_name: Some("Jane Example".into()),
            first_name: Some("Jane".into()),
            last_name: Some("Example".into()),
            signed_in_at: "2026-01-02T03:04:05Z".into(),
        }
    }

    #[test]
    fn a_blank_service_names_none() {
        assert_eq!(service_override(None), None);
        assert_eq!(service_override(Some("  ")), None);
        assert_eq!(
            service_override(Some("piekstra.fertemr.hermetic-tests")),
            Some("piekstra.fertemr.hermetic-tests")
        );
    }

    #[test]
    fn each_profile_has_its_own_entry() {
        let dir = temp_dir("entries");
        let keys = Arc::new(MemoryKeys::new());
        let store = SessionStore::with_vault(Vault::new(keys.clone(), &dir));
        assert!(store.get(&key("adult1")).unwrap().is_none());
        assert_eq!(keys.reads(), 0, "nothing stored, nothing read");
        assert_eq!(key("adult1").to_string(), "adult1/someclinic/adult1");

        store.put(&key("adult1"), &dummy()).unwrap();
        let read = store.get(&key("adult1")).unwrap().unwrap();
        assert_eq!(read.session_id.expose(), "dummy-session-0001");
        assert_eq!(read.device_id, "00000000-0000-4000-8000-000000000001");
        assert_eq!(read.patient_id, Some(2));
        assert_eq!(read.signed_in_at, "2026-01-02T03:04:05Z");
        assert_eq!(read.full_name.as_deref(), Some("Jane Example"));
        assert!(store.get(&key("adult2")).unwrap().is_none());

        for file in std::fs::read_dir(&dir).unwrap() {
            let bytes = std::fs::read(file.unwrap().path()).unwrap();
            assert!(!String::from_utf8_lossy(&bytes).contains("dummy-session"));
        }
        assert!(store.delete(&key("adult1")).unwrap());
        assert!(!store.delete(&key("adult1")).unwrap());
        assert!(store.load(&key("adult1")).err().unwrap().is_auth());
        assert_eq!(store.clear().unwrap(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_unreadable_entry_is_named_not_quoted() {
        let dir = temp_dir("unreadable");
        let vault = Vault::new(MemoryKeys::new(), &dir);
        vault
            .put("adult1/someclinic/adult1", &Secret::new(r#"{"token":"dummy_secret"}"#))
            .unwrap();
        vault
            .put(
                "adult2/someclinic/adult2",
                &Secret::new(r#"{"portal":"x","api_base":"x","device_id":"x","session_id":"","signed_in_at":"x"}"#),
            )
            .unwrap();
        let store = SessionStore::with_vault(vault);
        for name in ["adult1", "adult2"] {
            let e = store.get(&key(name)).err().unwrap();
            assert_eq!(e.reason(), Some(Reason::CredentialUnreadable), "{name}");
            assert!(!e.to_string().contains("dummy"), "{e}");
            assert!(
                e.remedy().unwrap().contains(&format!("auth logout --profile {name}")),
                "{e}"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_lost_key_names_auth_reset() {
        let dir = temp_dir("lost");
        let keys = Arc::new(MemoryKeys::new());
        let store = SessionStore::with_vault(Vault::new(keys.clone(), &dir));
        store.put(&key("adult1"), &dummy()).unwrap();
        keys.forget();
        let fresh = SessionStore::with_vault(Vault::new(keys, &dir));
        let e = fresh.get(&key("adult1")).err().unwrap();
        assert!(e.remedy().unwrap().contains("auth reset"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
