//! Account-switching orchestration: tying the credential store, the
//! `~/.claude.json` config, and the profile store together to save and
//! activate profiles.
//!
//! This is pure decision logic composed over the [`crate::creds`],
//! [`crate::config`], and [`crate::store`] ports, so it is unit-tested with a
//! fake credential store and temp directories — no real Keychain or process
//! spawning involved.
//!
//! ## The auth-loss fix
//!
//! The Claude Code OAuth refresh token rotates per **account**, shared across
//! every organization a single login can operate in (org selection is
//! client-side state in `~/.claude.json`, not part of the credential). The
//! original tool re-snapshotted the live credential into only the one profile
//! whose `(accountUuid, organizationUuid)` matched the live config. So two
//! profiles for the same login but different orgs each held their own copy of
//! the one shared token; the first refresh under either org rotated the shared
//! token and left the sibling profile holding a dead one — a delayed forced
//! re-login.
//!
//! [`Switcher`] closes that gap: before switching away it re-snapshots the
//! live credential into **every** profile that shares the outgoing account's
//! token, as selected by the configured [`TokenScope`]. Under
//! [`TokenScope::PerAccount`] that is all same-login profiles regardless of
//! org, so a rotation under any org keeps every sibling current.

use crate::config;
use crate::creds::CredentialStore;
use crate::error::{Error, Result};
use crate::model::Account;
use crate::store::{Store, TokenScope};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Orchestrates saving and activating profiles over the three ports.
pub struct Switcher<'a> {
    creds: &'a dyn CredentialStore,
    store: &'a Store,
    config_path: PathBuf,
    scope: TokenScope,
}

impl<'a> Switcher<'a> {
    /// Build a switcher over a credential store, a profile store, the path to
    /// `~/.claude.json`, and the token-sharing scope that drives the fix.
    pub fn new(
        creds: &'a dyn CredentialStore,
        store: &'a Store,
        config_path: impl Into<PathBuf>,
        scope: TokenScope,
    ) -> Self {
        Self {
            creds,
            store,
            config_path: config_path.into(),
            scope,
        }
    }

    /// The `~/.claude.json` path this switcher operates on.
    #[must_use]
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    /// Snapshot the live account into profile `name`.
    ///
    /// Reads the active credential blob, the keychain account attribute, and
    /// the `{oauthAccount, userID}` identity out of `~/.claude.json`, and
    /// writes them into the profile.
    ///
    /// If a profile of that name already exists this errors unless `force` is
    /// set, in which case it is overwritten (the `--force` behavior).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] when the profile exists and `force` is
    /// false, or when there is no active credential to snapshot; propagates
    /// config-read and store-write errors otherwise.
    pub fn save(&self, name: &str, force: bool) -> Result<Account> {
        if self.store.contains(name) && !force {
            return Err(Error::Invalid(format!(
                "profile '{name}' already exists (pass --force to overwrite)"
            )));
        }
        let blob = self.creds.read()?.ok_or_else(|| {
            Error::Invalid("no active credential found — sign in with 'claude' first".to_string())
        })?;
        let config = config::load(&self.config_path)?;
        let snapshot = self.identity_snapshot(&config)?;
        self.store.save_profile(name, &blob, &snapshot)?;
        Ok(config::account_of(&config))
    }

    /// Activate profile `name` as the current account.
    ///
    /// First re-snapshots the outgoing account (its token may have rotated)
    /// into every profile that shares its token, then restores `name`'s
    /// credential into the platform store and splices its identity into
    /// `~/.claude.json`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ProfileNotFound`] / [`Error::Invalid`] when the target
    /// profile is missing or incomplete; propagates credential, config, and
    /// store errors otherwise.
    pub fn activate(&self, name: &str) -> Result<Account> {
        // Validate the target up front so a missing/incomplete profile fails
        // before we touch any live state.
        let target = self.store.load(name)?;

        // Re-snapshot the outgoing account before overwriting it. This may
        // update `name`'s own credential file too, when it shares the outgoing
        // account's token — which is exactly what we want.
        self.sync_current()?;

        // Restore the credential. Re-read it from the store so we pick up any
        // rotation `sync_current` just propagated into this profile.
        let blob = self.store.load_credentials(name)?;
        self.creds.write(&blob, &target.keychain_account())?;

        // Splice this profile's identity (incl. organizationUuid) into
        // ~/.claude.json, preserving all unrelated keys.
        config::patch(&self.config_path, &target.identity())?;
        Ok(target.account())
    }

    /// Re-snapshot the live account's credential into every profile that
    /// shares its token, and refresh the identity of the exact profile it maps
    /// to. Silently does nothing when there is no config, no live identity, or
    /// no active credential.
    ///
    /// # Errors
    ///
    /// Propagates store-write errors.
    pub fn sync_current(&self) -> Result<()> {
        let Ok(config) = config::load(&self.config_path) else {
            return Ok(());
        };
        let active = config::account_of(&config);
        if !self.scope.identifies(&active) {
            return Ok(());
        }
        let Some(blob) = self.creds.read()? else {
            return Ok(());
        };
        let live_snapshot = self.identity_snapshot(&config)?;
        let active_key = active.key();

        for shared in self.store.profiles_sharing_token(&active, self.scope)? {
            // Every sibling gets the freshly rotated credential...
            self.store.write_credentials(&shared.name, &blob)?;
            // ...but only the profile whose (account, org) *is* the live one
            // gets its identity snapshot refreshed. A sibling for a different
            // org must keep its own organizationUuid.
            if shared.account.key() == active_key {
                self.store.write_account(&shared.name, &live_snapshot)?;
            }
        }
        Ok(())
    }

    /// Build the `{oauthAccount, userID, keychain_account}` snapshot stored in
    /// a profile's `account.json` from a loaded config and the current
    /// keychain account attribute.
    fn identity_snapshot(&self, config: &Value) -> Result<Value> {
        let keychain_account = self.creds.account_attr()?.unwrap_or_default();
        let mut snapshot = config::extract_identity(config);
        if let Some(obj) = snapshot.as_object_mut() {
            obj.insert("keychain_account".to_string(), json!(keychain_account));
        }
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::fs;
    use std::path::PathBuf;

    /// In-memory fake of the platform credential store.
    struct FakeCredentialStore {
        blob: RefCell<Option<String>>,
        acct: Option<String>,
        fail_read: bool,
        fail_write: bool,
        fail_acct: bool,
    }

    impl FakeCredentialStore {
        fn with(blob: Option<&str>, acct: Option<&str>) -> Self {
            Self {
                blob: RefCell::new(blob.map(str::to_string)),
                acct: acct.map(str::to_string),
                fail_read: false,
                fail_write: false,
                fail_acct: false,
            }
        }
    }

    impl CredentialStore for FakeCredentialStore {
        fn read(&self) -> Result<Option<String>> {
            if self.fail_read {
                return Err(Error::Invalid("read failed".to_string()));
            }
            Ok(self.blob.borrow().clone())
        }

        fn write(&self, blob: &str, _acct: &str) -> Result<()> {
            if self.fail_write {
                return Err(Error::Invalid("write failed".to_string()));
            }
            *self.blob.borrow_mut() = Some(blob.to_string());
            Ok(())
        }

        fn account_attr(&self) -> Result<Option<String>> {
            if self.fail_acct {
                return Err(Error::Invalid("account_attr failed".to_string()));
            }
            Ok(self.acct.clone())
        }
    }

    fn temp_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "ccswitch-switch-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&base).unwrap();
        base
    }

    fn config_json(acct: &str, org: &str, email: &str, user: &str) -> Value {
        json!({
            "numStartups": 3,
            "userID": user,
            "oauthAccount": {
                "accountUuid": acct,
                "organizationUuid": org,
                "emailAddress": email,
                "organizationName": "Org",
                "organizationRole": "admin",
                "profileFetchedAt": "yesterday"
            },
            "projects": { "/x": { "allowedTools": [] } }
        })
    }

    fn write_config(dir: &Path, value: &Value) -> PathBuf {
        let path = dir.join(".claude.json");
        fs::write(&path, serde_json::to_string_pretty(value).unwrap()).unwrap();
        path
    }

    // ---- save --------------------------------------------------------------

    #[test]
    fn save_new_profile_snapshots_live_state() {
        let dir = temp_dir();
        let config = write_config(&dir, &config_json("A", "O1", "a@example.com", "uid-1"));
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCredentialStore::with(Some("BLOB-1"), Some("login-attr"));
        let switcher = Switcher::new(&creds, &store, &config, TokenScope::PerAccount);

        let account = switcher.save("orga", false).unwrap();
        assert_eq!(account.email, "a@example.com");
        assert_eq!(switcher.config_path(), config);

        let loaded = store.load("orga").unwrap();
        assert_eq!(loaded.credentials, "BLOB-1");
        assert_eq!(loaded.keychain_account(), "login-attr");
        assert_eq!(loaded.account().org_uuid, "O1");
        assert_eq!(loaded.account.get("userID").unwrap(), &json!("uid-1"));
        // The full oauthAccount is preserved, not just keyed fields.
        assert_eq!(
            loaded.account["oauthAccount"]["profileFetchedAt"],
            json!("yesterday")
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_existing_without_force_errors() {
        let dir = temp_dir();
        let config = write_config(&dir, &config_json("A", "O1", "a@x", "uid-1"));
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCredentialStore::with(Some("BLOB-1"), None);
        let switcher = Switcher::new(&creds, &store, &config, TokenScope::PerAccount);

        switcher.save("orga", false).unwrap();
        let err = switcher.save("orga", false).unwrap_err();
        assert!(matches!(err, Error::Invalid(msg) if msg.contains("already exists")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_existing_with_force_overwrites() {
        let dir = temp_dir();
        let config = write_config(&dir, &config_json("A", "O1", "a@x", "uid-1"));
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCredentialStore::with(Some("BLOB-1"), None);
        let switcher = Switcher::new(&creds, &store, &config, TokenScope::PerAccount);
        switcher.save("orga", false).unwrap();

        // A newer credential arrives; force overwrites the stored snapshot.
        *creds.blob.borrow_mut() = Some("BLOB-2".to_string());
        switcher.save("orga", true).unwrap();
        assert_eq!(store.load("orga").unwrap().credentials, "BLOB-2");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_without_credential_errors() {
        let dir = temp_dir();
        let config = write_config(&dir, &config_json("A", "O1", "a@x", "uid-1"));
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCredentialStore::with(None, None);
        let switcher = Switcher::new(&creds, &store, &config, TokenScope::PerAccount);
        let err = switcher.save("orga", false).unwrap_err();
        assert!(matches!(err, Error::Invalid(msg) if msg.contains("no active credential")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_propagates_credential_read_error() {
        let dir = temp_dir();
        let config = write_config(&dir, &config_json("A", "O1", "a@x", "uid-1"));
        let store = Store::new(dir.join("accounts"));
        let mut creds = FakeCredentialStore::with(Some("BLOB"), None);
        creds.fail_read = true;
        let switcher = Switcher::new(&creds, &store, &config, TokenScope::PerAccount);
        assert!(switcher.save("orga", false).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_propagates_account_attr_error() {
        let dir = temp_dir();
        let config = write_config(&dir, &config_json("A", "O1", "a@x", "uid-1"));
        let store = Store::new(dir.join("accounts"));
        let mut creds = FakeCredentialStore::with(Some("BLOB"), None);
        creds.fail_acct = true;
        let switcher = Switcher::new(&creds, &store, &config, TokenScope::PerAccount);
        assert!(switcher.save("orga", false).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_missing_config_errors() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCredentialStore::with(Some("BLOB"), None);
        let switcher = Switcher::new(
            &creds,
            &store,
            dir.join(".claude.json"),
            TokenScope::PerAccount,
        );
        assert!(switcher.save("orga", false).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    // ---- activate ----------------------------------------------------------

    #[test]
    fn activate_restores_credential_and_identity() {
        let dir = temp_dir();
        // Live account is B/OB; we switch to a saved A/OA profile.
        let config = write_config(&dir, &config_json("B", "OB", "b@example.com", "uid-b"));
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCredentialStore::with(Some("LIVE-B"), Some("attr-b"));
        let switcher = Switcher::new(&creds, &store, &config, TokenScope::PerAccount);

        // Seed a saved profile for a different account.
        store
            .save_profile(
                "orga",
                "TOKEN-A",
                &json!({
                    "oauthAccount": {
                        "accountUuid": "A",
                        "organizationUuid": "OA",
                        "emailAddress": "a@example.com",
                        "organizationName": "Org A"
                    },
                    "userID": "uid-a",
                    "keychain_account": "attr-a"
                }),
            )
            .unwrap();

        let account = switcher.activate("orga").unwrap();
        assert_eq!(account.email, "a@example.com");

        // Credential store now holds profile A's token.
        assert_eq!(creds.blob.borrow().as_deref(), Some("TOKEN-A"));
        // Config identity was spliced to A/OA, unrelated keys preserved.
        let patched = config::load(&config).unwrap();
        assert_eq!(patched["oauthAccount"]["accountUuid"], json!("A"));
        assert_eq!(patched["oauthAccount"]["organizationUuid"], json!("OA"));
        assert_eq!(patched["userID"], json!("uid-a"));
        assert_eq!(patched["numStartups"], json!(3));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn activate_missing_profile_errors_without_touching_state() {
        let dir = temp_dir();
        let config = write_config(&dir, &config_json("A", "OA", "a@x", "uid-a"));
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCredentialStore::with(Some("LIVE"), None);
        let switcher = Switcher::new(&creds, &store, &config, TokenScope::PerAccount);
        assert!(matches!(
            switcher.activate("ghost"),
            Err(Error::ProfileNotFound(_))
        ));
        // Live credential untouched.
        assert_eq!(creds.blob.borrow().as_deref(), Some("LIVE"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn activate_propagates_credential_write_error() {
        let dir = temp_dir();
        let config = write_config(&dir, &config_json("B", "OB", "b@x", "uid-b"));
        let store = Store::new(dir.join("accounts"));
        store
            .save_profile(
                "orga",
                "TOKEN-A",
                &json!({
                    "oauthAccount": { "accountUuid": "A", "organizationUuid": "OA" },
                    "userID": "uid-a",
                    "keychain_account": "attr-a"
                }),
            )
            .unwrap();
        let mut creds = FakeCredentialStore::with(Some("LIVE-B"), Some("attr-b"));
        creds.fail_write = true;
        let switcher = Switcher::new(&creds, &store, &config, TokenScope::PerAccount);
        assert!(switcher.activate("orga").is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    // ---- the rotation fix --------------------------------------------------

    #[test]
    fn rotated_token_propagates_to_all_sibling_org_profiles() {
        let dir = temp_dir();
        // Two profiles: same account A, different orgs O1 and O2.
        // Active account is A/O1; both were saved with the same original token.
        let config = write_config(&dir, &config_json("A", "O1", "a@example.com", "uid-a"));
        let store = Store::new(dir.join("accounts"));

        let snap = |org: &str| {
            json!({
                "oauthAccount": {
                    "accountUuid": "A",
                    "organizationUuid": org,
                    "emailAddress": "a@example.com",
                    "organizationName": org
                },
                "userID": "uid-a",
                "keychain_account": "attr-a"
            })
        };
        store.save_profile("orga", "R-OLD", &snap("O1")).unwrap();
        store.save_profile("orgb", "R-OLD", &snap("O2")).unwrap();

        // The live credential has since rotated (claude ran under O1).
        let creds = FakeCredentialStore::with(Some("R-NEW"), Some("attr-a"));
        let switcher = Switcher::new(&creds, &store, &config, TokenScope::PerAccount);

        // Switch to the sibling org profile.
        switcher.activate("orgb").unwrap();

        // THE FIX: both same-account profiles now hold the rotated token, not
        // just the one matching the live (account, org) key.
        assert_eq!(store.load_credentials("orga").unwrap(), "R-NEW");
        assert_eq!(store.load_credentials("orgb").unwrap(), "R-NEW");

        // orga keeps its own org identity (not overwritten with O2's live one).
        assert_eq!(store.load("orga").unwrap().account().org_uuid, "O1");

        // The credential restored into the live store is the current token, so
        // switching to orgb does not force a re-login.
        assert_eq!(creds.blob.borrow().as_deref(), Some("R-NEW"));

        // And switching back to orga also restores the live token — no
        // re-login there either.
        switcher.activate("orga").unwrap();
        assert_eq!(creds.blob.borrow().as_deref(), Some("R-NEW"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn per_account_org_scope_isolates_credential_to_exact_profile() {
        let dir = temp_dir();
        let config = write_config(&dir, &config_json("A", "O1", "a@x", "uid-a"));
        let store = Store::new(dir.join("accounts"));
        let snap = |org: &str| {
            json!({
                "oauthAccount": { "accountUuid": "A", "organizationUuid": org },
                "userID": "uid-a",
                "keychain_account": "attr-a"
            })
        };
        store.save_profile("orga", "R-OLD", &snap("O1")).unwrap();
        store.save_profile("orgb", "R-OLD", &snap("O2")).unwrap();

        let creds = FakeCredentialStore::with(Some("R-NEW"), Some("attr-a"));
        // Under the stricter (buggy) scope, only the exact match is synced.
        let switcher = Switcher::new(&creds, &store, &config, TokenScope::PerAccountOrg);
        switcher.sync_current().unwrap();

        assert_eq!(store.load_credentials("orga").unwrap(), "R-NEW");
        // orgb is NOT updated — this is the stranding the PerAccount fix cures.
        assert_eq!(store.load_credentials("orgb").unwrap(), "R-OLD");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sync_current_refreshes_identity_of_exact_match() {
        let dir = temp_dir();
        // Live identity has advanced (new profileFetchedAt) vs the snapshot.
        let config = write_config(&dir, &config_json("A", "O1", "a@example.com", "uid-a"));
        let store = Store::new(dir.join("accounts"));
        store
            .save_profile(
                "orga",
                "R-OLD",
                &json!({
                    "oauthAccount": {
                        "accountUuid": "A",
                        "organizationUuid": "O1",
                        "profileFetchedAt": "stale"
                    },
                    "userID": "uid-a",
                    "keychain_account": "attr-a"
                }),
            )
            .unwrap();
        let creds = FakeCredentialStore::with(Some("R-NEW"), Some("attr-a"));
        let switcher = Switcher::new(&creds, &store, &config, TokenScope::PerAccount);
        switcher.sync_current().unwrap();

        let refreshed = store.load("orga").unwrap();
        assert_eq!(refreshed.credentials, "R-NEW");
        assert_eq!(
            refreshed.account["oauthAccount"]["profileFetchedAt"],
            json!("yesterday")
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sync_current_no_config_is_noop() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCredentialStore::with(Some("R-NEW"), None);
        let switcher = Switcher::new(
            &creds,
            &store,
            dir.join(".claude.json"),
            TokenScope::PerAccount,
        );
        switcher.sync_current().unwrap();
        assert!(store.list().unwrap().is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sync_current_empty_identity_is_noop() {
        let dir = temp_dir();
        let config = write_config(&dir, &json!({ "numStartups": 1 }));
        let store = Store::new(dir.join("accounts"));
        store
            .save_profile("orga", "R-OLD", &json!({ "oauthAccount": {} }))
            .unwrap();
        let creds = FakeCredentialStore::with(Some("R-NEW"), None);
        let switcher = Switcher::new(&creds, &store, &config, TokenScope::PerAccount);
        switcher.sync_current().unwrap();
        assert_eq!(store.load_credentials("orga").unwrap(), "R-OLD");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sync_current_no_live_credential_is_noop() {
        let dir = temp_dir();
        let config = write_config(&dir, &config_json("A", "O1", "a@x", "uid-a"));
        let store = Store::new(dir.join("accounts"));
        store
            .save_profile(
                "orga",
                "R-OLD",
                &json!({ "oauthAccount": { "accountUuid": "A", "organizationUuid": "O1" } }),
            )
            .unwrap();
        let creds = FakeCredentialStore::with(None, None);
        let switcher = Switcher::new(&creds, &store, &config, TokenScope::PerAccount);
        switcher.sync_current().unwrap();
        assert_eq!(store.load_credentials("orga").unwrap(), "R-OLD");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sync_current_propagates_credential_read_error() {
        let dir = temp_dir();
        let config = write_config(&dir, &config_json("A", "O1", "a@x", "uid-a"));
        let store = Store::new(dir.join("accounts"));
        let mut creds = FakeCredentialStore::with(Some("R-NEW"), None);
        creds.fail_read = true;
        let switcher = Switcher::new(&creds, &store, &config, TokenScope::PerAccount);
        assert!(switcher.sync_current().is_err());
        fs::remove_dir_all(&dir).unwrap();
    }
}
