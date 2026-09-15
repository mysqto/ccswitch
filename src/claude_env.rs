//! Where Claude Code keeps its state, as a function of the environment.
//!
//! `$CLAUDE_CONFIG_DIR` does not merely move `~/.claude`: it also moves
//! `.claude.json`, and — the part that is easy to miss — it *renames the
//! Keychain item*. Claude Code namespaces the service by appending the first
//! eight hex characters of `sha256($CLAUDE_CONFIG_DIR)`:
//!
//! ```text
//! unset            → "Claude Code-credentials"
//! /Users/x/.work   → "Claude Code-credentials-744b5d0e"
//! ```
//!
//! So a `ccswitch` that ignores the variable does not just read the wrong
//! config file — it reads and writes a *different account's* Keychain item.
//! That matters in ordinary use, because `ccswitch isolate` launches `claude`
//! with `CLAUDE_CONFIG_DIR` set to the isolate directory; running `ccswitch`
//! from inside such a session used to swap the main account's credential.
//!
//! Two related variables come along for the ride, because they feed the same
//! computation: `$CLAUDE_SECURESTORAGE_CONFIG_DIR` relocates only the
//! credential store, and `$CLAUDE_CODE_CUSTOM_OAUTH_URL` gives the config file
//! and the Keychain service a `-custom-oauth` infix.
//!
//! ## Normalization
//!
//! Claude Code normalizes the directory to Unicode NFC before hashing.
//! `ccswitch` hashes the bytes as given, which agrees for any path already in
//! NFC — every ASCII path, and in practice everything short of a
//! decomposed-form path typed on macOS. A decomposed non-ASCII
//! `$CLAUDE_CONFIG_DIR` would hash differently; set `$CCSWITCH_CREDENTIALS` to
//! `file` there, or use an ASCII path.

use crate::sha256;
use std::path::{Path, PathBuf};

/// The Claude Code environment variables that move its state around.
///
/// Each field holds the raw variable: `None` when unset. An empty string is
/// treated as unset for [`ClaudeEnv::config_dir`], but is meaningful for
/// `secure_storage_dir`, where Claude Code reads "defined but empty" as an
/// explicit request for the default store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaudeEnv {
    /// `$CLAUDE_CONFIG_DIR`.
    pub config_dir: Option<String>,
    /// `$CLAUDE_SECURESTORAGE_CONFIG_DIR`.
    pub secure_storage_dir: Option<String>,
    /// `$CLAUDE_CODE_CUSTOM_OAUTH_URL`, reduced to whether it is set.
    pub custom_oauth: bool,
}

/// The base Keychain service name and the infix for a custom OAuth endpoint.
const SERVICE_STEM: &str = "Claude Code";
const SERVICE_LEAF: &str = "-credentials";
const CUSTOM_OAUTH_SUFFIX: &str = "-custom-oauth";

impl ClaudeEnv {
    /// Build from already-read variables, treating empty strings as unset
    /// except where Claude Code gives them meaning.
    #[must_use]
    pub fn new(
        config_dir: Option<String>,
        secure_storage_dir: Option<String>,
        custom_oauth_url: Option<String>,
    ) -> Self {
        Self {
            config_dir: config_dir.filter(|v| !v.is_empty()),
            secure_storage_dir,
            custom_oauth: custom_oauth_url.is_some_and(|v| !v.is_empty()),
        }
    }

    /// The infix that a custom OAuth endpoint adds to the config file name and
    /// the Keychain service.
    #[must_use]
    pub fn oauth_suffix(&self) -> &'static str {
        if self.custom_oauth {
            CUSTOM_OAUTH_SUFFIX
        } else {
            ""
        }
    }

    /// Claude Code's configuration directory: `$CLAUDE_CONFIG_DIR`, else
    /// `~/.claude`. This is the directory holding `projects/`, `plugins/`, and
    /// the plaintext credential fallback.
    #[must_use]
    pub fn config_dir(&self, home: &Path) -> PathBuf {
        self.config_dir
            .as_ref()
            .map_or_else(|| home.join(".claude"), PathBuf::from)
    }

    /// The global config file Claude Code validates the credential against.
    ///
    /// Note it sits *beside* the config directory by default (`~/.claude.json`,
    /// not `~/.claude/.claude.json`), but *inside* `$CLAUDE_CONFIG_DIR` when
    /// that is set.
    #[must_use]
    pub fn config_path(&self, home: &Path) -> PathBuf {
        let base = self
            .config_dir
            .as_ref()
            .map_or_else(|| home.to_path_buf(), PathBuf::from);
        base.join(format!(".claude{}.json", self.oauth_suffix()))
    }

    /// The directory holding the plaintext `.credentials.json` fallback:
    /// `$CLAUDE_SECURESTORAGE_CONFIG_DIR` when set to a non-empty value,
    /// `~/.claude` when it is set but empty, else [`ClaudeEnv::config_dir`].
    #[must_use]
    pub fn credentials_dir(&self, home: &Path) -> PathBuf {
        match self.secure_storage_dir.as_deref() {
            Some("") => home.join(".claude"),
            Some(dir) => PathBuf::from(dir),
            None => self.config_dir(home),
        }
    }

    /// The Keychain service name the credential is stored under.
    ///
    /// The default store is unsuffixed; any relocation namespaces it by the
    /// first eight hex characters of the directory's SHA-256.
    #[must_use]
    pub fn keychain_service(&self) -> String {
        let scope = match self.secure_storage_dir.as_deref() {
            // Defined but empty means "the default store", not a new namespace.
            Some("") => None,
            Some(dir) => Some(dir),
            None => self.config_dir.as_deref(),
        };
        let suffix = scope.map_or_else(String::new, |dir| {
            format!("-{}", &sha256::hex_digest(dir.as_bytes())[..8])
        });
        format!(
            "{SERVICE_STEM}{}{SERVICE_LEAF}{suffix}",
            self.oauth_suffix()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/Users/someone")
    }

    fn env(config_dir: Option<&str>) -> ClaudeEnv {
        ClaudeEnv::new(config_dir.map(str::to_string), None, None)
    }

    #[test]
    fn defaults_match_a_plain_install() {
        let e = env(None);
        assert_eq!(
            e.config_dir(&home()),
            PathBuf::from("/Users/someone/.claude")
        );
        // Beside the directory, not inside it.
        assert_eq!(
            e.config_path(&home()),
            PathBuf::from("/Users/someone/.claude.json")
        );
        assert_eq!(
            e.credentials_dir(&home()),
            PathBuf::from("/Users/someone/.claude")
        );
        assert_eq!(e.keychain_service(), "Claude Code-credentials");
        assert_eq!(e.oauth_suffix(), "");
    }

    #[test]
    fn config_dir_moves_everything_including_the_keychain_item() {
        let e = env(Some("/Users/someone/.claude-work"));
        assert_eq!(
            e.config_dir(&home()),
            PathBuf::from("/Users/someone/.claude-work")
        );
        // The config file moves *inside* the directory here.
        assert_eq!(
            e.config_path(&home()),
            PathBuf::from("/Users/someone/.claude-work/.claude.json")
        );
        assert_eq!(
            e.credentials_dir(&home()),
            PathBuf::from("/Users/someone/.claude-work")
        );
        let expected = format!(
            "Claude Code-credentials-{}",
            &sha256::hex_digest(b"/Users/someone/.claude-work")[..8]
        );
        assert_eq!(e.keychain_service(), expected);
        // And it is genuinely a different item from the default one.
        assert_ne!(e.keychain_service(), env(None).keychain_service());
    }

    #[test]
    fn empty_config_dir_is_treated_as_unset() {
        let e = env(Some(""));
        assert_eq!(
            e.config_dir(&home()),
            PathBuf::from("/Users/someone/.claude")
        );
        assert_eq!(e.keychain_service(), "Claude Code-credentials");
    }

    #[test]
    fn secure_storage_dir_moves_only_the_credential_store() {
        let e = ClaudeEnv::new(Some("/cfg".to_string()), Some("/secure".to_string()), None);
        // Config still follows CLAUDE_CONFIG_DIR...
        assert_eq!(e.config_path(&home()), PathBuf::from("/cfg/.claude.json"));
        // ...while the credential store follows the storage variable.
        assert_eq!(e.credentials_dir(&home()), PathBuf::from("/secure"));
        let expected = format!(
            "Claude Code-credentials-{}",
            &sha256::hex_digest(b"/secure")[..8]
        );
        assert_eq!(e.keychain_service(), expected);
    }

    #[test]
    fn secure_storage_dir_set_but_empty_selects_the_default_store() {
        // Claude Code reads "defined but empty" as an explicit opt-out of the
        // namespacing that CLAUDE_CONFIG_DIR would otherwise apply.
        let e = ClaudeEnv::new(Some("/cfg".to_string()), Some(String::new()), None);
        assert_eq!(
            e.credentials_dir(&home()),
            PathBuf::from("/Users/someone/.claude")
        );
        assert_eq!(e.keychain_service(), "Claude Code-credentials");
    }

    #[test]
    fn custom_oauth_url_infixes_the_config_file_and_the_service() {
        let e = ClaudeEnv::new(None, None, Some("https://example.test/oauth".to_string()));
        assert_eq!(e.oauth_suffix(), "-custom-oauth");
        assert_eq!(
            e.config_path(&home()),
            PathBuf::from("/Users/someone/.claude-custom-oauth.json")
        );
        assert_eq!(e.keychain_service(), "Claude Code-custom-oauth-credentials");
        // Empty means unset.
        let e = ClaudeEnv::new(None, None, Some(String::new()));
        assert_eq!(e.oauth_suffix(), "");
    }

    #[test]
    fn is_debuggable_and_comparable() {
        let e = env(Some("/x"));
        assert_eq!(e.clone(), e);
        assert_ne!(ClaudeEnv::default(), e);
        assert!(format!("{e:?}").contains("ClaudeEnv"));
    }
}
