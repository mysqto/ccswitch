//! Credential port: reading and writing the Claude Code OAuth credential
//! blob from the platform store.
//!
//! The [`CredentialStore`] trait and all decision logic (path selection,
//! empty handling) live here, together with the file-based implementation
//! used on Linux and Windows. The real macOS Keychain adapter, which shells
//! out to the `security` binary, lives in [`crate::creds_shim`] and is
//! excluded from coverage.

use crate::error::Result;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// Abstraction over the platform credential store holding the Claude Code
/// OAuth credential blob.
pub trait CredentialStore {
    /// Return the stored credential blob, or `None` when no credential is
    /// present (missing or empty).
    ///
    /// # Errors
    ///
    /// Returns an error if the store cannot be read.
    fn read(&self) -> Result<Option<String>>;

    /// Write `blob` into the store under account attribute `acct`.
    ///
    /// The `acct` attribute is only meaningful for the macOS Keychain; the
    /// file store ignores it.
    ///
    /// # Errors
    ///
    /// Returns an error if the store cannot be written.
    fn write(&self, blob: &str, acct: &str) -> Result<()>;

    /// Return the account attribute the credential is stored under, or
    /// `None` when the store does not track one (the file store).
    ///
    /// # Errors
    ///
    /// Returns an error if the store cannot be queried.
    fn account_attr(&self) -> Result<Option<String>>;
}

/// File-based credential store: keeps the blob in
/// `<config_dir>/.credentials.json`.
///
/// This is the fallback used on every platform other than macOS. It is pure
/// filesystem I/O under a configurable directory, so it is unit-testable
/// against a temporary directory without a shim.
#[derive(Debug, Clone)]
pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    /// Create a store rooted at `config_dir`.
    pub fn new(config_dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: config_dir.into(),
        }
    }

    /// The path of the credential file this store reads and writes.
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.dir.join(".credentials.json")
    }
}

impl CredentialStore for FileStore {
    fn read(&self) -> Result<Option<String>> {
        match fs::read_to_string(self.path()) {
            Ok(blob) if blob.trim().is_empty() => Ok(None),
            Ok(blob) => Ok(Some(blob)),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    fn write(&self, blob: &str, _acct: &str) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        let path = self.path();
        fs::write(&path, blob)?;
        set_permissions_600(&path)?;
        Ok(())
    }

    fn account_attr(&self) -> Result<Option<String>> {
        Ok(None)
    }
}

/// Tighten a file's permissions to owner read/write only.
#[cfg(unix)]
pub(crate) fn set_permissions_600(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

/// No-op on platforms without Unix permission bits.
#[cfg(not(unix))]
pub(crate) fn set_permissions_600(_path: &Path) -> Result<()> {
    Ok(())
}

/// Tighten a directory's permissions to owner access only.
#[cfg(unix)]
pub(crate) fn set_permissions_700(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

/// No-op on platforms without Unix permission bits.
#[cfg(not(unix))]
pub(crate) fn set_permissions_700(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "ccswitch-creds-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn path_is_credentials_json_under_dir() {
        let store = FileStore::new("/some/config/dir");
        assert_eq!(
            store.path(),
            PathBuf::from("/some/config/dir/.credentials.json")
        );
    }

    #[test]
    fn read_missing_file_is_none() {
        let dir = temp_dir();
        let store = FileStore::new(&dir);
        assert_eq!(store.read().unwrap(), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn read_empty_file_is_none() {
        let dir = temp_dir();
        let store = FileStore::new(&dir);
        fs::write(store.path(), "   \n").unwrap();
        assert_eq!(store.read().unwrap(), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = temp_dir();
        let store = FileStore::new(&dir);
        store.write("{\"token\":\"abc\"}", "ignored").unwrap();
        assert_eq!(
            store.read().unwrap().as_deref(),
            Some("{\"token\":\"abc\"}")
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_creates_missing_parent_directory() {
        let dir = temp_dir();
        let nested = dir.join("deep/nested/config");
        let store = FileStore::new(&nested);
        store.write("blob", "").unwrap();
        assert!(nested.join(".credentials.json").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn write_sets_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir();
        let store = FileStore::new(&dir);
        store.write("blob", "").unwrap();
        let mode = fs::metadata(store.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn read_propagates_unexpected_error() {
        let dir = temp_dir();
        let store = FileStore::new(&dir);
        // Make the credential path a directory so reading it as a string
        // fails with an error other than "not found".
        fs::create_dir_all(store.path()).unwrap();
        assert!(store.read().is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_fails_when_dir_uncreatable() {
        // A directory nested under a regular file cannot be created.
        let dir = temp_dir();
        let blocker = dir.join("blocker");
        fs::write(&blocker, "x").unwrap();
        let store = FileStore::new(blocker.join("sub"));
        assert!(store.write("blob", "").is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_fails_when_target_is_directory() {
        let dir = temp_dir();
        let store = FileStore::new(&dir);
        // The credential path is a directory, so writing the blob fails.
        fs::create_dir_all(store.path()).unwrap();
        assert!(store.write("blob", "").is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn set_permissions_error_on_missing_path() {
        let dir = temp_dir();
        let missing = dir.join("nope");
        assert!(set_permissions_600(&missing).is_err());
        assert!(set_permissions_700(&missing).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn account_attr_is_none_for_file_store() {
        let store = FileStore::new("/anywhere");
        assert_eq!(store.account_attr().unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn set_permissions_700_tightens_directory() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir();
        let nested = dir.join("locked");
        fs::create_dir_all(&nested).unwrap();
        set_permissions_700(&nested).unwrap();
        let mode = fs::metadata(&nested).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn store_is_debuggable_and_cloneable() {
        let store = FileStore::new("/x");
        let cloned = store.clone();
        assert_eq!(store.path(), cloned.path());
        assert!(format!("{store:?}").contains("FileStore"));
    }
}
