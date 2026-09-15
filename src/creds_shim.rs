//! Real credential-store adapter (macOS `security` binary).
//!
//! Side-effecting implementation of the [`crate::creds::Keychain`] port. This
//! is the only place in the crate that runs the `security` binary, and it is
//! excluded from coverage. It contains no decision logic: the single method
//! spawns `security` with the arguments it is handed, optionally feeding it
//! stdin, and returns the raw exit status and streams for
//! [`crate::creds`] to classify. Which subcommand runs, under which service
//! name, and whether the secret travels on stdin or argv are all decided
//! there; the store itself is assembled by [`crate::creds::select_store`].

use crate::creds::{select_store, Backend, CredentialStore};
#[cfg(target_os = "macos")]
use crate::creds::{Keychain, KeychainOutput};
#[cfg(target_os = "macos")]
use crate::error::Result;
use std::path::PathBuf;

/// Select the credential store for the current platform and environment: the
/// macOS Keychain (backed by the plaintext fallback) when available, otherwise
/// the file-based store rooted at `credentials_dir`.
///
/// `service` is the Keychain service name, which
/// [`crate::claude_env::ClaudeEnv::keychain_service`] derives from
/// `$CLAUDE_CONFIG_DIR`.
#[must_use]
pub fn platform_store(
    credentials_dir: PathBuf,
    service: String,
    default_account: String,
) -> Box<dyn CredentialStore> {
    let backend = Backend::from_env(std::env::var("CCSWITCH_CREDENTIALS").ok().as_deref());
    #[cfg(target_os = "macos")]
    let keychain: Option<Box<dyn Keychain>> = Some(Box::new(SecurityBinary));
    #[cfg(not(target_os = "macos"))]
    let keychain = None;
    select_store(backend, keychain, credentials_dir, service, default_account)
}

/// macOS Keychain adapter backed by the `security` binary.
#[cfg(target_os = "macos")]
pub struct SecurityBinary;

#[cfg(target_os = "macos")]
impl Keychain for SecurityBinary {
    fn run(&self, args: &[String], stdin: Option<&str>) -> Result<KeychainOutput> {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let mut command = Command::new("security");
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            });
        let mut child = command.spawn()?;
        if let Some(input) = stdin {
            if let Some(mut pipe) = child.stdin.take() {
                pipe.write_all(input.as_bytes())?;
            }
        }
        let output = child.wait_with_output()?;
        Ok(KeychainOutput {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}
