//! Real credential-store adapter (macOS `security` binary).
//!
//! Side-effecting implementation of the [`crate::creds`] port. This is the
//! only place in the crate that runs the `security` binary, and it is
//! excluded from coverage. It contains no decision logic: platform selection
//! is a single `cfg` switch and each method is a thin wrapper over one
//! `security` invocation.

use crate::creds::CredentialStore;
#[cfg(not(target_os = "macos"))]
use crate::creds::FileStore;
#[cfg(target_os = "macos")]
use crate::error::Error;
use crate::error::Result;
use std::path::PathBuf;

/// Service name under which Claude Code stores its OAuth credential.
#[cfg(target_os = "macos")]
const SERVICE: &str = "Claude Code-credentials";

/// Select the credential store for the current platform: the macOS Keychain
/// when available, otherwise the file-based fallback rooted at `config_dir`.
#[must_use]
pub fn platform_store(config_dir: PathBuf) -> Box<dyn CredentialStore> {
    #[cfg(target_os = "macos")]
    {
        let _ = config_dir;
        Box::new(KeychainStore)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(FileStore::new(config_dir))
    }
}

/// macOS Keychain credential store backed by the `security` binary.
#[cfg(target_os = "macos")]
pub struct KeychainStore;

#[cfg(target_os = "macos")]
impl CredentialStore for KeychainStore {
    fn read(&self) -> Result<Option<String>> {
        use std::process::Command;
        let output = Command::new("security")
            .args(["find-generic-password", "-s", SERVICE, "-w"])
            .output()?;
        if !output.status.success() {
            return Ok(None);
        }
        let blob = String::from_utf8_lossy(&output.stdout)
            .trim_end_matches('\n')
            .to_string();
        if blob.is_empty() {
            Ok(None)
        } else {
            Ok(Some(blob))
        }
    }

    fn write(&self, blob: &str, acct: &str) -> Result<()> {
        use std::process::Command;
        let acct = if acct.is_empty() {
            std::env::var("USER").unwrap_or_default()
        } else {
            acct.to_string()
        };
        let _ = Command::new("security")
            .args(["delete-generic-password", "-s", SERVICE])
            .output();
        let status = Command::new("security")
            .args([
                "add-generic-password",
                "-U",
                "-a",
                &acct,
                "-s",
                SERVICE,
                "-w",
                blob,
            ])
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(Error::Invalid("failed to write credential".to_string()))
        }
    }

    fn account_attr(&self) -> Result<Option<String>> {
        use std::process::Command;
        let output = Command::new("security")
            .args(["find-generic-password", "-s", SERVICE, "-g"])
            .output()?;
        let text = String::from_utf8_lossy(&output.stderr);
        for line in text.lines() {
            if let Some(rest) = line.trim().strip_prefix("\"acct\"<blob>=\"") {
                if let Some(acct) = rest.strip_suffix('"') {
                    return Ok(Some(acct.to_string()));
                }
            }
        }
        Ok(None)
    }
}
