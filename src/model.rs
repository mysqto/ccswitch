//! Core data types describing Claude Code accounts and saved profiles.

use serde::{Deserialize, Serialize};

/// Identity of a single Claude Code account within an organization.
///
/// A login can span multiple organizations, so an account is uniquely
/// identified by the pair (`account_uuid`, `org_uuid`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// Stable per-user account identifier (`oauthAccount.accountUuid`).
    #[serde(rename = "accountUuid", default)]
    pub account_uuid: String,

    /// Organization identifier (`oauthAccount.organizationUuid`).
    #[serde(rename = "organizationUuid", default)]
    pub org_uuid: String,

    /// Account email address (`oauthAccount.emailAddress`).
    #[serde(rename = "emailAddress", default)]
    pub email: String,

    /// Human-readable organization name (`oauthAccount.organizationName`).
    #[serde(rename = "organizationName", default)]
    pub org: String,
}

impl Account {
    /// The identity key used to compare two accounts for equality when
    /// deciding whether a profile is the currently active one.
    #[must_use]
    pub fn key(&self) -> String {
        format!("{}|{}", self.account_uuid, self.org_uuid)
    }
}

/// Metadata persisted alongside a saved profile's credentials.
///
/// Mirrors the `account.json` file written for each profile: the account
/// identity, the Claude Code `userID`, and the keychain account attribute
/// used to restore the credential on macOS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// The account identity for this profile.
    #[serde(rename = "oauthAccount")]
    pub account: Account,

    /// Claude Code's opaque user identifier.
    #[serde(rename = "userID", default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,

    /// The macOS keychain account attribute the credential was stored under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keychain_account: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_account() -> Account {
        Account {
            account_uuid: "acct-123".to_string(),
            org_uuid: "org-456".to_string(),
            email: "person@example.com".to_string(),
            org: "Example Org".to_string(),
        }
    }

    #[test]
    fn account_key_joins_identity() {
        assert_eq!(sample_account().key(), "acct-123|org-456");
    }

    #[test]
    fn account_round_trips_via_claude_field_names() {
        let json = r#"{
            "accountUuid": "acct-123",
            "organizationUuid": "org-456",
            "emailAddress": "person@example.com",
            "organizationName": "Example Org"
        }"#;
        let parsed: Account = serde_json::from_str(json).unwrap();
        assert_eq!(parsed, sample_account());

        let reserialized = serde_json::to_string(&parsed).unwrap();
        let reparsed: Account = serde_json::from_str(&reserialized).unwrap();
        assert_eq!(reparsed, parsed);
    }

    #[test]
    fn account_defaults_missing_fields() {
        let parsed: Account = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed.key(), "|");
        assert!(parsed.email.is_empty());
        assert!(parsed.org.is_empty());
    }

    #[test]
    fn account_is_debuggable_and_cloneable() {
        let account = sample_account();
        let cloned = account.clone();
        assert_eq!(account, cloned);
        assert!(format!("{account:?}").contains("acct-123"));
    }

    #[test]
    fn profile_round_trips() {
        let profile = Profile {
            account: sample_account(),
            user_id: Some("user-789".to_string()),
            keychain_account: Some("login".to_string()),
        };
        let json = serde_json::to_string(&profile).unwrap();
        let parsed: Profile = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, profile);
        assert!(format!("{profile:?}").contains("user-789"));
    }

    #[test]
    fn profile_omits_absent_optionals() {
        let profile = Profile {
            account: sample_account(),
            user_id: None,
            keychain_account: None,
        };
        let json = serde_json::to_string(&profile).unwrap();
        assert!(!json.contains("userID"));
        assert!(!json.contains("keychain_account"));

        let parsed: Profile = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, profile);
    }
}
