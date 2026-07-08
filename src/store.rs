//! The on-disk profile store: enumerating, saving, and removing profiles
//! under the accounts directory (`$CCSWITCH_HOME`, default
//! `~/.claude/accounts`).
//!
//! Each profile is a directory holding two files:
//!
//! * `credentials.json` — the raw Claude Code OAuth credential blob, exactly
//!   as read from the platform credential store.
//! * `account.json` — the identity snapshot `{oauthAccount, userID,
//!   keychain_account}` spliced out of `~/.claude.json`. The full
//!   `oauthAccount` object is preserved verbatim as JSON so that restoring a
//!   profile carries back every field (organizationUuid, tiers, onboarding
//!   flags, …), not just the handful this crate keys on.
//!
//! This is plain-file I/O under a configurable root, so it is unit-tested
//! against a temporary directory with no shim.

use crate::config;
use crate::creds::{set_permissions_600, set_permissions_700};
use crate::error::{Error, Result};
use crate::model::Account;
use serde_json::Value;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// File name of the credential blob within a profile directory.
pub const CREDENTIALS_FILE: &str = "credentials.json";

/// File name of the identity snapshot within a profile directory.
pub const ACCOUNT_FILE: &str = "account.json";

/// How broadly a single, rotating OAuth credential is shared.
///
/// This is the knob that fixes the auth-loss bug: the Claude Code refresh
/// token rotates per *account*, shared across every organization a login can
/// see. Keying a stored credential by the full `(account, org)` pair — as the
/// original tool did — means two profiles for the same login but different
/// orgs each believe they own a separate token, when in truth there is one.
/// The first refresh under either org rotates the shared token and strands the
/// sibling with a dead one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenScope {
    /// One rotating credential per login, shared across all of its orgs.
    /// Profiles are grouped by `accountUuid` alone.
    PerAccount,
    /// One credential per `(accountUuid, organizationUuid)` pair.
    PerAccountOrg,
}

impl TokenScope {
    /// The key grouping profiles that share one credential under this scope.
    #[must_use]
    pub fn token_key(self, account: &Account) -> String {
        match self {
            TokenScope::PerAccount => account.account_uuid.clone(),
            TokenScope::PerAccountOrg => account.key(),
        }
    }

    /// Whether `account` carries enough identity to select a token group.
    ///
    /// An all-empty identity (no `accountUuid`, no `organizationUuid`) matches
    /// nothing, so credential propagation is skipped rather than blindly
    /// overwriting every profile.
    #[must_use]
    pub fn identifies(self, account: &Account) -> bool {
        match self {
            TokenScope::PerAccount => !account.account_uuid.is_empty(),
            TokenScope::PerAccountOrg => {
                !account.account_uuid.is_empty() || !account.org_uuid.is_empty()
            }
        }
    }
}

/// A profile loaded from disk: its credential blob and identity snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredProfile {
    /// The raw OAuth credential blob (`credentials.json`).
    pub credentials: String,
    /// The identity snapshot object (`account.json`).
    pub account: Value,
}

impl StoredProfile {
    /// The account identity keyed on, parsed out of the snapshot.
    #[must_use]
    pub fn account(&self) -> Account {
        config::account_of(&self.account)
    }

    /// The `{oauthAccount, userID}` slice to splice back into `~/.claude.json`.
    #[must_use]
    pub fn identity(&self) -> Value {
        config::extract_identity(&self.account)
    }

    /// The keychain account attribute the credential should be restored under.
    #[must_use]
    pub fn keychain_account(&self) -> String {
        self.account
            .get("keychain_account")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// A profile listed by name alongside its parsed identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSummary {
    /// The profile's directory name.
    pub name: String,
    /// The account identity read from its `account.json`.
    pub account: Account,
}

/// The profile store rooted at a single directory.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// Create a store rooted at `root` (the `$CCSWITCH_HOME` directory).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The store's root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The directory holding profile `name`.
    #[must_use]
    pub fn profile_dir(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn credentials_path(&self, name: &str) -> PathBuf {
        self.profile_dir(name).join(CREDENTIALS_FILE)
    }

    fn account_path(&self, name: &str) -> PathBuf {
        self.profile_dir(name).join(ACCOUNT_FILE)
    }

    /// Whether a profile directory named `name` already exists.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.profile_dir(name).exists()
    }

    /// Create the profile directory (and the root) with owner-only perms.
    fn ensure_dir(&self, name: &str) -> Result<PathBuf> {
        let dir = self.profile_dir(name);
        fs::create_dir_all(&dir)?;
        set_permissions_700(&self.root)?;
        set_permissions_700(&dir)?;
        Ok(dir)
    }

    /// Write only the credential blob of profile `name`.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory or file cannot be created/written.
    pub fn write_credentials(&self, name: &str, blob: &str) -> Result<()> {
        self.ensure_dir(name)?;
        let path = self.credentials_path(name);
        fs::write(&path, blob)?;
        set_permissions_600(&path)?;
        Ok(())
    }

    /// Write only the identity snapshot of profile `name`.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory or file cannot be created/written.
    pub fn write_account(&self, name: &str, account: &Value) -> Result<()> {
        self.ensure_dir(name)?;
        let path = self.account_path(name);
        let text = serde_json::to_string_pretty(account)?;
        fs::write(&path, text)?;
        set_permissions_600(&path)?;
        Ok(())
    }

    /// Save both files of a profile in one step.
    ///
    /// # Errors
    ///
    /// Returns an error if either file cannot be written.
    pub fn save_profile(&self, name: &str, blob: &str, account: &Value) -> Result<()> {
        self.write_credentials(name, blob)?;
        self.write_account(name, account)?;
        Ok(())
    }

    /// Load profile `name`, both files.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ProfileNotFound`] when no such directory exists, or
    /// [`Error::Invalid`] when the directory is missing one of its two files.
    pub fn load(&self, name: &str) -> Result<StoredProfile> {
        if !self.contains(name) {
            return Err(Error::ProfileNotFound(name.to_string()));
        }
        let credentials = self.load_credentials(name)?;
        let account_text = read_required(&self.account_path(name), name)?;
        let account = serde_json::from_str(&account_text)?;
        Ok(StoredProfile {
            credentials,
            account,
        })
    }

    /// Load just the credential blob of profile `name`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ProfileNotFound`] when the directory is absent, or
    /// [`Error::Invalid`] when the directory exists without a credential file.
    pub fn load_credentials(&self, name: &str) -> Result<String> {
        if !self.contains(name) {
            return Err(Error::ProfileNotFound(name.to_string()));
        }
        read_required(&self.credentials_path(name), name)
    }

    /// List every saved profile (directories with an `account.json`), sorted
    /// by name.
    ///
    /// # Errors
    ///
    /// Returns an error if the root cannot be read (other than being absent,
    /// which yields an empty list) or a snapshot is not valid JSON.
    pub fn list(&self) -> Result<Vec<ProfileSummary>> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err.into()),
        };
        let mut out = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let account_text = match fs::read_to_string(self.account_path(&name)) {
                Ok(text) => text,
                Err(err) if err.kind() == ErrorKind::NotFound => continue,
                Err(err) => return Err(err.into()),
            };
            let snapshot: Value = serde_json::from_str(&account_text)?;
            out.push(ProfileSummary {
                name,
                account: config::account_of(&snapshot),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Every profile that shares a rotating credential with `active` under
    /// `scope`.
    ///
    /// With [`TokenScope::PerAccount`] this returns *all* profiles of the same
    /// login regardless of org — the set that must be kept in lockstep so a
    /// rotation under one org does not strand the others. An identity that
    /// does not [`TokenScope::identifies`] anything yields an empty list.
    ///
    /// # Errors
    ///
    /// Returns an error if the store cannot be listed.
    pub fn profiles_sharing_token(
        &self,
        active: &Account,
        scope: TokenScope,
    ) -> Result<Vec<ProfileSummary>> {
        if !scope.identifies(active) {
            return Ok(Vec::new());
        }
        let key = scope.token_key(active);
        Ok(self
            .list()?
            .into_iter()
            .filter(|summary| {
                scope.identifies(&summary.account) && scope.token_key(&summary.account) == key
            })
            .collect())
    }

    /// Remove profile `name` and all of its files.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ProfileNotFound`] when no such directory exists.
    pub fn remove(&self, name: &str) -> Result<()> {
        if !self.contains(name) {
            return Err(Error::ProfileNotFound(name.to_string()));
        }
        fs::remove_dir_all(self.profile_dir(name))?;
        Ok(())
    }
}

/// Read a file that a profile is required to have, mapping a missing file to
/// an "incomplete profile" error rather than a bare I/O error.
fn read_required(path: &Path, name: &str) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == ErrorKind::NotFound => {
            Err(Error::Invalid(format!("profile '{name}' is incomplete")))
        }
        Err(err) => Err(err.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "ccswitch-store-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&base).unwrap();
        base
    }

    fn account_snapshot(acct: &str, org: &str, email: &str) -> Value {
        json!({
            "oauthAccount": {
                "accountUuid": acct,
                "organizationUuid": org,
                "emailAddress": email,
                "organizationName": "Org",
                "organizationRole": "admin"
            },
            "userID": "user-hash",
            "keychain_account": "login-attr"
        })
    }

    #[test]
    fn token_scope_keys_group_by_scope() {
        let account = Account {
            account_uuid: "A".to_string(),
            org_uuid: "O".to_string(),
            ..Account::default()
        };
        assert_eq!(TokenScope::PerAccount.token_key(&account), "A");
        assert_eq!(TokenScope::PerAccountOrg.token_key(&account), "A|O");
    }

    #[test]
    fn token_scope_identifies_requires_identity() {
        let empty = Account::default();
        assert!(!TokenScope::PerAccount.identifies(&empty));
        assert!(!TokenScope::PerAccountOrg.identifies(&empty));

        let only_org = Account {
            org_uuid: "O".to_string(),
            ..Account::default()
        };
        assert!(!TokenScope::PerAccount.identifies(&only_org));
        assert!(TokenScope::PerAccountOrg.identifies(&only_org));

        let with_acct = Account {
            account_uuid: "A".to_string(),
            ..Account::default()
        };
        assert!(TokenScope::PerAccount.identifies(&with_acct));
        assert!(TokenScope::PerAccountOrg.identifies(&with_acct));
    }

    #[test]
    fn token_scope_is_copy_and_debuggable() {
        let scope = TokenScope::PerAccount;
        let copied = scope;
        assert_eq!(scope, copied);
        assert!(format!("{scope:?}").contains("PerAccount"));
    }

    #[test]
    fn paths_are_under_root() {
        let store = Store::new("/root");
        assert_eq!(store.root(), Path::new("/root"));
        assert_eq!(store.profile_dir("dev"), PathBuf::from("/root/dev"));
        assert_eq!(
            store.credentials_path("dev"),
            PathBuf::from("/root/dev/credentials.json")
        );
        assert_eq!(
            store.account_path("dev"),
            PathBuf::from("/root/dev/account.json")
        );
    }

    #[test]
    fn store_is_cloneable_and_debuggable() {
        let store = Store::new("/root");
        let cloned = store.clone();
        assert_eq!(store.root(), cloned.root());
        assert!(format!("{store:?}").contains("Store"));
    }

    #[test]
    fn save_then_load_round_trips_both_files() {
        let dir = temp_dir();
        let store = Store::new(&dir);
        let snapshot = account_snapshot("A", "O", "a@example.com");
        store.save_profile("dev", "BLOB", &snapshot).unwrap();

        assert!(store.contains("dev"));
        let loaded = store.load("dev").unwrap();
        assert_eq!(loaded.credentials, "BLOB");
        assert_eq!(loaded.account, snapshot);
        // The full oauthAccount object is preserved, not just keyed fields.
        assert_eq!(
            loaded.account["oauthAccount"]["organizationRole"],
            json!("admin")
        );
        assert_eq!(loaded.account().account_uuid, "A");
        assert_eq!(loaded.identity()["userID"], json!("user-hash"));
        assert_eq!(loaded.keychain_account(), "login-attr");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stored_profile_keychain_account_defaults_empty() {
        let profile = StoredProfile {
            credentials: "x".to_string(),
            account: json!({ "oauthAccount": {} }),
        };
        assert_eq!(profile.keychain_account(), "");
        assert_eq!(
            profile,
            StoredProfile {
                credentials: "x".to_string(),
                account: json!({ "oauthAccount": {} }),
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn saved_files_and_dirs_have_tight_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir();
        let store = Store::new(&dir);
        store
            .save_profile("dev", "BLOB", &account_snapshot("A", "O", "a@x"))
            .unwrap();
        let mode = |p: PathBuf| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(store.profile_dir("dev")), 0o700);
        assert_eq!(mode(store.credentials_path("dev")), 0o600);
        assert_eq!(mode(store.account_path("dev")), 0o600);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_missing_profile_is_not_found() {
        let dir = temp_dir();
        let store = Store::new(&dir);
        assert!(matches!(
            store.load("ghost"),
            Err(Error::ProfileNotFound(name)) if name == "ghost"
        ));
        assert!(matches!(
            store.load_credentials("ghost"),
            Err(Error::ProfileNotFound(_))
        ));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn list_unreadable_root_errors() {
        // A root that is a file (not a directory) surfaces a read_dir error
        // other than "not found".
        let dir = temp_dir();
        let file_root = dir.join("iam-a-file");
        fs::write(&file_root, "x").unwrap();
        let store = Store::new(&file_root);
        assert!(store.list().is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn list_unreadable_account_errors() {
        // An account.json that is a directory fails to read as a string with
        // an error other than "not found".
        let dir = temp_dir();
        let store = Store::new(&dir);
        fs::create_dir_all(store.account_path("dev")).unwrap();
        assert!(store.list().is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_credentials_unreadable_file_errors() {
        // A credentials.json that is a directory hits the non-NotFound arm of
        // the required-file reader.
        let dir = temp_dir();
        let store = Store::new(&dir);
        fs::create_dir_all(store.credentials_path("dev")).unwrap();
        // Give it an account.json so the directory counts as a profile.
        store
            .write_account("dev", &account_snapshot("A", "O", "a@x"))
            .unwrap();
        assert!(store.load_credentials("dev").is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_incomplete_profile_errors() {
        let dir = temp_dir();
        let store = Store::new(&dir);
        // Directory exists with only the account file, no credentials.
        store
            .write_account("dev", &account_snapshot("A", "O", "a@x"))
            .unwrap();
        assert!(matches!(
            store.load_credentials("dev"),
            Err(Error::Invalid(msg)) if msg.contains("incomplete")
        ));
        assert!(matches!(store.load("dev"), Err(Error::Invalid(_))));

        // And with only credentials, no account file.
        let store2 = Store::new(dir.join("other"));
        store2.write_credentials("dev", "BLOB").unwrap();
        assert!(matches!(store2.load("dev"), Err(Error::Invalid(_))));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_malformed_account_json_errors() {
        let dir = temp_dir();
        let store = Store::new(&dir);
        store.write_credentials("dev", "blob").unwrap();
        fs::write(store.account_path("dev"), "{not json").unwrap();
        assert!(store.load("dev").is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_fails_when_directory_uncreatable() {
        // Rooting the store under a regular file makes the profile directory
        // impossible to create.
        let dir = temp_dir();
        let blocker = dir.join("blocker");
        fs::write(&blocker, "x").unwrap();
        let store = Store::new(&blocker);
        assert!(store.write_credentials("dev", "blob").is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_credentials_fails_when_target_is_directory() {
        let dir = temp_dir();
        let store = Store::new(&dir);
        fs::create_dir_all(store.credentials_path("dev")).unwrap();
        assert!(store.write_credentials("dev", "blob").is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_account_fails_when_target_is_directory() {
        let dir = temp_dir();
        let store = Store::new(&dir);
        fs::create_dir_all(store.account_path("dev")).unwrap();
        assert!(store.write_account("dev", &json!({})).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn list_empty_root_is_empty() {
        let dir = temp_dir();
        let missing = dir.join("nope");
        let store = Store::new(&missing);
        assert!(store.list().unwrap().is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn list_skips_non_dirs_and_dirs_without_account() {
        let dir = temp_dir();
        let store = Store::new(&dir);
        store
            .save_profile("bravo", "b", &account_snapshot("B", "O", "b@x"))
            .unwrap();
        store
            .save_profile("alpha", "a", &account_snapshot("A", "O", "a@x"))
            .unwrap();
        // A stray file at the root is not a profile.
        fs::write(dir.join("loose.txt"), "junk").unwrap();
        // A directory without an account.json is not a profile.
        fs::create_dir_all(dir.join("half")).unwrap();

        let names: Vec<String> = store.list().unwrap().into_iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["alpha".to_string(), "bravo".to_string()]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn list_malformed_account_json_errors() {
        let dir = temp_dir();
        let store = Store::new(&dir);
        fs::create_dir_all(store.profile_dir("dev")).unwrap();
        fs::write(store.account_path("dev"), "{not json").unwrap();
        assert!(store.list().is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn profiles_sharing_token_groups_by_account_under_per_account() {
        let dir = temp_dir();
        let store = Store::new(&dir);
        // Two orgs of one login, plus an unrelated account.
        store
            .save_profile("orga", "t", &account_snapshot("A", "O1", "a@x"))
            .unwrap();
        store
            .save_profile("orgb", "t", &account_snapshot("A", "O2", "a@x"))
            .unwrap();
        store
            .save_profile("other", "t", &account_snapshot("Z", "O9", "z@x"))
            .unwrap();

        let active = Account {
            account_uuid: "A".to_string(),
            org_uuid: "O1".to_string(),
            ..Account::default()
        };

        let per_account: Vec<String> = store
            .profiles_sharing_token(&active, TokenScope::PerAccount)
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(per_account, vec!["orga".to_string(), "orgb".to_string()]);

        let per_org: Vec<String> = store
            .profiles_sharing_token(&active, TokenScope::PerAccountOrg)
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(per_org, vec!["orga".to_string()]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn profiles_sharing_token_empty_identity_matches_nothing() {
        let dir = temp_dir();
        let store = Store::new(&dir);
        store
            .save_profile("orga", "t", &account_snapshot("A", "O1", "a@x"))
            .unwrap();
        let empty = Account::default();
        assert!(store
            .profiles_sharing_token(&empty, TokenScope::PerAccount)
            .unwrap()
            .is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn remove_deletes_profile() {
        let dir = temp_dir();
        let store = Store::new(&dir);
        store
            .save_profile("dev", "t", &account_snapshot("A", "O", "a@x"))
            .unwrap();
        assert!(store.contains("dev"));
        store.remove("dev").unwrap();
        assert!(!store.contains("dev"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn remove_missing_profile_is_not_found() {
        let dir = temp_dir();
        let store = Store::new(&dir);
        assert!(matches!(
            store.remove("ghost"),
            Err(Error::ProfileNotFound(_))
        ));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn profile_summary_is_debuggable_and_cloneable() {
        let summary = ProfileSummary {
            name: "dev".to_string(),
            account: Account::default(),
        };
        let cloned = summary.clone();
        assert_eq!(summary, cloned);
        assert!(format!("{summary:?}").contains("dev"));
    }
}
