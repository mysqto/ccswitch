//! Reading and splicing the Claude Code `~/.claude.json` configuration file.
//!
//! Claude Code keeps a large amount of unrelated state in `~/.claude.json`
//! alongside the signed-in identity (`oauthAccount` and `userID`). To switch
//! accounts we must swap only that identity in and out while leaving every
//! other key untouched. All of that is plain-file JSON manipulation under a
//! caller-provided path, so it lives here and is unit-tested against a
//! temporary file with no shim.

use crate::creds::set_permissions_600;
use crate::error::Result;
use crate::model::Account;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// The two config keys that make up a Claude Code identity snapshot.
const IDENTITY_KEYS: [&str; 2] = ["oauthAccount", "userID"];

/// Load `~/.claude.json` from `path` as a JSON value.
///
/// # Errors
///
/// Returns an error if the file is missing or is not valid JSON.
pub fn load(path: &Path) -> Result<Value> {
    let text = fs::read_to_string(path)?;
    let value = serde_json::from_str(&text)?;
    Ok(value)
}

/// Extract the identity snapshot (`{oauthAccount, userID}`) from a loaded
/// config value. Missing keys are captured as JSON `null` so the shape is
/// stable.
#[must_use]
pub fn extract_identity(config: &Value) -> Value {
    json!({
        "oauthAccount": config.get("oauthAccount").cloned().unwrap_or(Value::Null),
        "userID": config.get("userID").cloned().unwrap_or(Value::Null),
    })
}

/// Splice a saved identity snapshot back into `config`, replacing only the
/// identity keys and leaving every other key untouched. A key absent from
/// `identity` leaves the corresponding config key unchanged; a non-object
/// config is left as-is.
pub fn splice_identity(config: &mut Value, identity: &Value) {
    let Some(obj) = config.as_object_mut() else {
        return;
    };
    for key in IDENTITY_KEYS {
        if let Some(value) = identity.get(key) {
            obj.insert(key.to_string(), value.clone());
        }
    }
}

/// Read the account identity (accountUuid, organizationUuid, emailAddress,
/// organizationName) out of a loaded config value.
#[must_use]
pub fn account_of(config: &Value) -> Account {
    match config.get("oauthAccount") {
        Some(value) => serde_json::from_value(value.clone()).unwrap_or_default(),
        None => Account::default(),
    }
}

/// The `.ccswitch.bak` backup path for a config file.
#[must_use]
fn backup_path(config_path: &Path) -> PathBuf {
    let mut name = config_path.as_os_str().to_owned();
    name.push(".ccswitch.bak");
    PathBuf::from(name)
}

/// The `.ccswitch.tmp` scratch path used for atomic writes.
#[must_use]
fn temp_path(config_path: &Path) -> PathBuf {
    let mut name = config_path.as_os_str().to_owned();
    name.push(".ccswitch.tmp");
    PathBuf::from(name)
}

/// Patch the identity in the config file at `config_path` with `identity`,
/// preserving all other keys.
///
/// The existing file is backed up to `<path>.ccswitch.bak` first, and the new
/// contents are written atomically via a sibling temp file that is renamed
/// into place.
///
/// # Errors
///
/// Returns an error if the config cannot be read, backed up, or written.
pub fn patch(config_path: &Path, identity: &Value) -> Result<()> {
    let mut config = load(config_path)?;
    fs::copy(config_path, backup_path(config_path))?;
    splice_identity(&mut config, identity);
    let text = serde_json::to_string_pretty(&config)?;
    let tmp = temp_path(config_path);
    fs::write(&tmp, text.as_bytes())?;
    set_permissions_600(&tmp)?;
    fs::rename(&tmp, config_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "ccswitch-config-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&base).unwrap();
        base
    }

    fn sample_config() -> Value {
        json!({
            "numStartups": 7,
            "userID": "user-old",
            "oauthAccount": {
                "accountUuid": "acct-old",
                "organizationUuid": "org-old",
                "emailAddress": "old@example.com",
                "organizationName": "Old Org"
            },
            "projects": { "/tmp/foo": { "allowedTools": [] } }
        })
    }

    #[test]
    fn load_reads_json() {
        let dir = temp_dir();
        let path = dir.join(".claude.json");
        fs::write(&path, sample_config().to_string()).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded["numStartups"], json!(7));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_missing_file_errors() {
        let dir = temp_dir();
        let path = dir.join(".claude.json");
        assert!(load(&path).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_invalid_json_errors() {
        let dir = temp_dir();
        let path = dir.join(".claude.json");
        fs::write(&path, "{not json").unwrap();
        assert!(load(&path).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extract_identity_pulls_both_keys() {
        let identity = extract_identity(&sample_config());
        assert_eq!(identity["userID"], json!("user-old"));
        assert_eq!(
            identity["oauthAccount"]["emailAddress"],
            json!("old@example.com")
        );
    }

    #[test]
    fn extract_identity_defaults_missing_keys_to_null() {
        let identity = extract_identity(&json!({ "numStartups": 1 }));
        assert_eq!(identity["oauthAccount"], Value::Null);
        assert_eq!(identity["userID"], Value::Null);
    }

    #[test]
    fn splice_replaces_identity_and_preserves_others() {
        let mut config = sample_config();
        let identity = json!({
            "userID": "user-new",
            "oauthAccount": {
                "accountUuid": "acct-new",
                "organizationUuid": "org-new",
                "emailAddress": "new@example.com",
                "organizationName": "New Org"
            }
        });
        splice_identity(&mut config, &identity);
        assert_eq!(config["userID"], json!("user-new"));
        assert_eq!(config["oauthAccount"]["accountUuid"], json!("acct-new"));
        // Unrelated keys survive untouched.
        assert_eq!(config["numStartups"], json!(7));
        assert_eq!(config["projects"]["/tmp/foo"]["allowedTools"], json!([]));
    }

    #[test]
    fn splice_leaves_config_key_when_identity_key_absent() {
        let mut config = sample_config();
        let identity = json!({ "userID": "user-new" });
        splice_identity(&mut config, &identity);
        assert_eq!(config["userID"], json!("user-new"));
        // oauthAccount was not in identity, so it is left as it was.
        assert_eq!(config["oauthAccount"]["accountUuid"], json!("acct-old"));
    }

    #[test]
    fn splice_ignores_non_object_config() {
        let mut config = Value::Null;
        splice_identity(&mut config, &json!({ "userID": "x" }));
        assert_eq!(config, Value::Null);
    }

    #[test]
    fn account_of_reads_all_four_fields() {
        let account = account_of(&sample_config());
        assert_eq!(account.account_uuid, "acct-old");
        assert_eq!(account.org_uuid, "org-old");
        assert_eq!(account.email, "old@example.com");
        assert_eq!(account.org, "Old Org");
        assert_eq!(account.key(), "acct-old|org-old");
    }

    #[test]
    fn account_of_missing_oauth_is_default() {
        let account = account_of(&json!({ "numStartups": 1 }));
        assert_eq!(account, Account::default());
    }

    #[test]
    fn account_of_malformed_oauth_is_default() {
        let account = account_of(&json!({ "oauthAccount": "not-an-object" }));
        assert_eq!(account, Account::default());
    }

    #[test]
    fn patch_round_trips_identity_and_backs_up() {
        let dir = temp_dir();
        let path = dir.join(".claude.json");
        fs::write(&path, sample_config().to_string()).unwrap();

        let identity = extract_identity(&json!({
            "userID": "user-new",
            "oauthAccount": {
                "accountUuid": "acct-new",
                "organizationUuid": "org-new",
                "emailAddress": "new@example.com",
                "organizationName": "New Org"
            }
        }));
        patch(&path, &identity).unwrap();

        let patched = load(&path).unwrap();
        assert_eq!(patched["userID"], json!("user-new"));
        assert_eq!(
            patched["oauthAccount"]["emailAddress"],
            json!("new@example.com")
        );
        // Unrelated keys preserved through the atomic rewrite.
        assert_eq!(patched["numStartups"], json!(7));

        // A backup of the pre-patch file exists and holds the old identity.
        let backup = load(&backup_path(&path)).unwrap();
        assert_eq!(backup["userID"], json!("user-old"));

        // The temp scratch file was renamed away, not left behind.
        assert!(!temp_path(&path).exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn patch_missing_file_errors_before_backup() {
        let dir = temp_dir();
        let path = dir.join(".claude.json");
        assert!(patch(&path, &json!({ "userID": "x" })).is_err());
        assert!(!backup_path(&path).exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn patch_sets_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir();
        let path = dir.join(".claude.json");
        fs::write(&path, sample_config().to_string()).unwrap();
        patch(&path, &extract_identity(&sample_config())).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        fs::remove_dir_all(&dir).unwrap();
    }
}
