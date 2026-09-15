//! Credential port: reading and writing the Claude Code OAuth credential
//! blob from the platform store.
//!
//! The [`CredentialStore`] trait and all decision logic (backend selection,
//! Keychain-vs-file arbitration, `security` exit-code classification) live
//! here, together with the file-based [`FileStore`]. The real macOS adapter
//! is a thin [`Keychain`] port over the `security` binary in
//! [`crate::creds_shim`], which is excluded from coverage.
//!
//! ## Why the Keychain alone is not enough
//!
//! Claude Code does not store its credential in the macOS Keychain
//! unconditionally: its store is a *composite*. It reads the Keychain first
//! and falls back to plaintext `~/.claude/.credentials.json`, and when a
//! Keychain write fails it migrates the credential to that file instead. In a
//! pure-SSH or otherwise headless session the login Keychain cannot be
//! unlocked without a GUI prompt, `security` exits 36
//! (`errSecInteractionNotAllowed`), and Claude Code is therefore living
//! entirely in the plaintext file.
//!
//! A Keychain-only `ccswitch` is invisible to that: `read` sees nothing (so
//! `save` claims you are not signed in and `sync_current` silently drops a
//! rotated token), and `write` cannot land the incoming credential — so
//! `~/.claude.json` names the account you switched to while the live token
//! still belongs to the old one, and Claude Code reports you as signed out.
//! [`KeychainStore`] mirrors Claude Code's own arbitration so the two agree
//! in every environment.

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

    /// Explain why [`read`](CredentialStore::read) found nothing, when the
    /// store knows more than "it was absent" — e.g. an unreachable Keychain.
    ///
    /// Returns `None` when the store has nothing to add, in which case the
    /// caller uses its own generic wording.
    ///
    /// # Errors
    ///
    /// Returns an error if the store cannot be queried.
    fn unavailable_hint(&self) -> Result<Option<String>> {
        Ok(None)
    }
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

    /// Delete the credential file, treating an already-absent file as success.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be removed.
    pub fn remove(&self) -> Result<()> {
        match fs::remove_file(self.path()) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
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

// ---------------------------------------------------------------------------
// macOS Keychain, with the plaintext fallback Claude Code itself uses
// ---------------------------------------------------------------------------

/// `security` exit status when the requested item is simply not in the
/// keychain (`errSecItemNotFound`). A genuine "signed out", not a failure.
const SEC_ITEM_NOT_FOUND: i32 = 44;

/// Raw result of one `security` invocation. The [`Keychain`] port hands this
/// back unclassified so that every decision about it stays testable here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeychainOutput {
    /// Process exit status, or `None` when the process was killed by a signal.
    pub code: Option<i32>,
    /// Captured standard output (the blob, for `find-generic-password -w`).
    pub stdout: String,
    /// Captured standard error (the attribute dump, for `-g`).
    pub stderr: String,
}

impl KeychainOutput {
    /// Whether `security` exited cleanly.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.code == Some(0)
    }
}

/// Port over the macOS `security` binary: one raw invocation, no decisions.
///
/// Keeping this down to a single method puts *every* choice — which
/// subcommand, which service name, whether the secret travels on argv or
/// stdin — in [`KeychainStore`], where it is unit-testable against a fake that
/// records exactly what would have been run.
///
/// The real adapter lives in [`crate::creds_shim`].
pub trait Keychain {
    /// Run `security` with `args`, optionally feeding `stdin` to it.
    ///
    /// # Errors
    ///
    /// Returns an error if `security` cannot be spawned.
    fn run(&self, args: &[String], stdin: Option<&str>) -> Result<KeychainOutput>;
}

/// What the keychain had to say when asked for the credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeychainState {
    /// The credential is present.
    Found(String),
    /// The keychain is readable and holds no credential.
    Empty,
    /// The keychain could not be consulted at all — locked, absent, or
    /// requiring a GUI prompt that a headless/SSH session cannot show.
    Unavailable,
}

impl KeychainState {
    /// Classify a `find-generic-password -w` result.
    ///
    /// Exit 0 with output means the blob; exit 0 with nothing and
    /// [`SEC_ITEM_NOT_FOUND`] both mean "no credential stored"; anything else
    /// (notably 36, `errSecInteractionNotAllowed`) means the keychain itself
    /// is out of reach, which is a different situation entirely — the
    /// credential may well exist, we just cannot see it.
    #[must_use]
    pub fn classify(output: &KeychainOutput) -> Self {
        match output.code {
            Some(0) => {
                let blob = output.stdout.trim_end_matches('\n');
                if blob.is_empty() {
                    Self::Empty
                } else {
                    Self::Found(blob.to_string())
                }
            }
            Some(SEC_ITEM_NOT_FOUND) => Self::Empty,
            _ => Self::Unavailable,
        }
    }
}

/// Pull the `acct` attribute out of a `find-generic-password -g` dump, which
/// `security` prints on **stdout** as `"acct"<blob>="somebody"`. Only the
/// password itself goes to stderr, so parsing that stream (as this tool used
/// to) never matched and left every profile with an empty attribute.
#[must_use]
pub fn parse_account_attr(stderr: &str) -> Option<String> {
    stderr.lines().find_map(|line| {
        line.trim()
            .strip_prefix("\"acct\"<blob>=\"")
            .and_then(|rest| rest.strip_suffix('"'))
            .map(str::to_string)
    })
}

/// Longest `security -i` stdin command line Claude Code will use before
/// falling back to argv; mirrored so the two agree on the boundary.
const STDIN_COMMAND_LIMIT: usize = 4032;

/// Characters Claude Code accepts in a Keychain account attribute. Anything
/// else and it substitutes [`FALLBACK_ACCOUNT`], so we must too — otherwise
/// the two store the credential under different accounts.
const FALLBACK_ACCOUNT: &str = "claude-code-user";

/// Normalize an account attribute the way Claude Code does.
#[must_use]
pub fn sanitize_account(name: &str) -> String {
    let ok = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        name.to_string()
    } else {
        FALLBACK_ACCOUNT.to_string()
    }
}

/// Lowercase hex encoding, for `security -X`.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The macOS credential store: the login Keychain in front of the plaintext
/// `<credentials_dir>/.credentials.json` that Claude Code falls back to.
///
/// Read and write mirror Claude Code's own composite store so the two never
/// disagree about which copy is authoritative:
///
/// * **read** — the Keychain wins; anything else (empty *or* unreachable)
///   falls through to the file.
/// * **write** — try the Keychain and, on success, delete the file, because a
///   leftover plaintext copy is a stale credential waiting to be picked up the
///   next time the Keychain is unreachable. If the Keychain write fails, write
///   the file and drop the Keychain item so its stale copy cannot shadow us
///   from a GUI session.
///
/// The credential never appears in `security`'s argv: it is hex-encoded and
/// the whole command is piped to `security -i`, so it is not visible in `ps`
/// to other users on the machine. Only a blob too large for the stdin limit
/// falls back to argv, which is what Claude Code does as well.
pub struct KeychainStore {
    keychain: Box<dyn Keychain>,
    fallback: FileStore,
    service: String,
    default_account: String,
}

impl KeychainStore {
    /// Build a store over a [`Keychain`] adapter, the plaintext fallback, the
    /// Keychain service name (see [`crate::claude_env::ClaudeEnv`], which
    /// derives it from `$CLAUDE_CONFIG_DIR`), and the account attribute to use
    /// when a profile carries none — the OS username, which is what Claude
    /// Code looks items up by.
    #[must_use]
    pub fn new(
        keychain: Box<dyn Keychain>,
        fallback: FileStore,
        service: String,
        default_account: String,
    ) -> Self {
        Self {
            keychain,
            fallback,
            service,
            default_account,
        }
    }

    /// Run one `security` subcommand against this store's service.
    fn security(&self, verb: &str, extra: &[&str]) -> Result<KeychainOutput> {
        let mut args = vec![verb.to_string(), "-s".to_string(), self.service.clone()];
        args.extend(extra.iter().map(|a| (*a).to_string()));
        self.keychain.run(&args, None)
    }

    /// The current keychain state.
    fn state(&self) -> Result<KeychainState> {
        Ok(KeychainState::classify(
            &self.security("find-generic-password", &["-w"])?,
        ))
    }

    /// Store `blob` under `acct`, keeping it off argv when it fits.
    fn store_password(&self, blob: &str, acct: &str) -> Result<KeychainOutput> {
        let hex = hex_encode(blob.as_bytes());
        let script = format!(
            "add-generic-password -U -a \"{acct}\" -s \"{}\" -X \"{hex}\"\n",
            self.service
        );
        if script.len() <= STDIN_COMMAND_LIMIT {
            return self.keychain.run(&["-i".to_string()], Some(&script));
        }
        // Too long for one stdin command line; Claude Code drops to argv here
        // too. The secret is momentarily visible in `ps` on this path.
        let args = [
            "add-generic-password",
            "-U",
            "-a",
            acct,
            "-s",
            &self.service,
            "-X",
            &hex,
        ]
        .map(str::to_string);
        self.keychain.run(&args, None)
    }
}

impl CredentialStore for KeychainStore {
    fn read(&self) -> Result<Option<String>> {
        match self.state()? {
            KeychainState::Found(blob) => Ok(Some(blob)),
            // Empty *and* Unavailable fall through: Claude Code reads the
            // plaintext file in both cases, so an SSH session sees the
            // credential it is actually using.
            KeychainState::Empty | KeychainState::Unavailable => self.fallback.read(),
        }
    }

    fn write(&self, blob: &str, acct: &str) -> Result<()> {
        let acct = sanitize_account(if acct.is_empty() {
            &self.default_account
        } else {
            acct
        });
        // `security` keys an item by (service, account), so adding under a
        // different account attribute would leave a second item behind and
        // reads would pick an arbitrary one. Clear first.
        let _ = self.security("delete-generic-password", &[]);
        if self.store_password(blob, &acct)?.succeeded() {
            return self.fallback.remove();
        }
        self.fallback.write(blob, &acct)?;
        let _ = self.security("delete-generic-password", &[]);
        Ok(())
    }

    fn account_attr(&self) -> Result<Option<String>> {
        Ok(parse_account_attr(
            &self.security("find-generic-password", &["-g"])?.stdout,
        ))
    }

    fn unavailable_hint(&self) -> Result<Option<String>> {
        if self.state()? != KeychainState::Unavailable {
            return Ok(None);
        }
        Ok(Some(format!(
            "no active credential found — the macOS Keychain is locked or unreachable \\
             (the usual case over SSH with no GUI login) and there is no fallback at {}; \\
             unlock it with 'security unlock-keychain', or sign in with 'claude' in this \\
             session to write the fallback",
            self.fallback.path().display()
        )))
    }
}

/// Which credential backend to use, selected by `$CCSWITCH_CREDENTIALS`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Backend {
    /// Platform default: Keychain-with-file-fallback on macOS, file elsewhere.
    #[default]
    Auto,
    /// Force the plaintext file, skipping the Keychain entirely. Useful on a
    /// headless Mac where every `security` call is a dead end anyway.
    File,
}

impl Backend {
    /// Parse the `$CCSWITCH_CREDENTIALS` value; anything unrecognized (or
    /// unset) is [`Backend::Auto`].
    #[must_use]
    pub fn from_env(var: Option<&str>) -> Self {
        match var.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("file" | "plaintext") => Self::File,
            _ => Self::Auto,
        }
    }
}

/// Assemble the credential store for a platform and backend choice.
///
/// `keychain` is `Some` only on macOS; passing `None`, or asking for
/// [`Backend::File`], yields the plaintext [`FileStore`].
#[must_use]
pub fn select_store(
    backend: Backend,
    keychain: Option<Box<dyn Keychain>>,
    credentials_dir: PathBuf,
    service: String,
    default_account: String,
) -> Box<dyn CredentialStore> {
    match (backend, keychain) {
        (Backend::Auto, Some(keychain)) => Box::new(KeychainStore::new(
            keychain,
            FileStore::new(credentials_dir),
            service,
            default_account,
        )),
        _ => Box::new(FileStore::new(credentials_dir)),
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
    use std::cell::RefCell;
    use std::rc::Rc;

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

    // ---- the Keychain composite -------------------------------------------

    /// Scriptable [`Keychain`] fake. Records every invocation — argv *and*
    /// stdin — so tests can assert on what would really have been run.
    /// One recorded `security` invocation: its argv and its stdin.
    type CallLog = Rc<RefCell<Vec<(Vec<String>, Option<String>)>>>;

    struct FakeKeychain {
        find: KeychainOutput,
        attrs: KeychainOutput,
        store_ok: bool,
        calls: CallLog,
        fail_find: bool,
        fail_store: bool,
        fail_attrs: bool,
    }

    impl FakeKeychain {
        fn new(find: KeychainOutput) -> Self {
            Self {
                find,
                attrs: KeychainOutput::default(),
                store_ok: true,
                calls: Rc::new(RefCell::new(Vec::new())),
                fail_find: false,
                fail_store: false,
                fail_attrs: false,
            }
        }

        /// A handle on the call log that outlives boxing into the store.
        fn log(&self) -> CallLog {
            Rc::clone(&self.calls)
        }
    }

    fn exit(code: i32, stdout: &str) -> KeychainOutput {
        KeychainOutput {
            code: Some(code),
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    impl Keychain for FakeKeychain {
        fn run(&self, args: &[String], stdin: Option<&str>) -> Result<KeychainOutput> {
            self.calls
                .borrow_mut()
                .push((args.to_vec(), stdin.map(str::to_string)));
            let verb = args.first().map_or("", String::as_str);
            if verb == "-i" || verb == "add-generic-password" {
                if self.fail_store {
                    return Err(crate::error::Error::Invalid("spawn failed".to_string()));
                }
                return Ok(exit(i32::from(!self.store_ok), ""));
            }
            if verb == "delete-generic-password" {
                return Ok(exit(0, ""));
            }
            if args.iter().any(|a| a == "-g") {
                if self.fail_attrs {
                    return Err(crate::error::Error::Invalid("spawn failed".to_string()));
                }
                return Ok(self.attrs.clone());
            }
            if self.fail_find {
                return Err(crate::error::Error::Invalid("spawn failed".to_string()));
            }
            Ok(self.find.clone())
        }
    }

    const TEST_SERVICE: &str = "Claude Code-credentials";

    /// A `KeychainStore` over the fake, with its fallback in `dir`.
    fn composite(keychain: FakeKeychain, dir: &Path) -> KeychainStore {
        KeychainStore::new(
            Box::new(keychain),
            FileStore::new(dir),
            TEST_SERVICE.to_string(),
            "default-user".to_string(),
        )
    }

    /// Flatten a call log to `argv` strings for easy assertions.
    fn argv(log: &CallLog) -> Vec<String> {
        log.borrow().iter().map(|(a, _)| a.join(" ")).collect()
    }

    /// The stdin script of the most recent call.
    fn last_script(log: &CallLog) -> String {
        log.borrow().last().unwrap().1.clone().unwrap()
    }

    #[test]
    fn classify_maps_every_security_outcome() {
        assert_eq!(
            KeychainState::classify(&exit(0, "BLOB\n")),
            KeychainState::Found("BLOB".to_string())
        );
        assert_eq!(KeychainState::classify(&exit(0, "")), KeychainState::Empty);
        assert_eq!(KeychainState::classify(&exit(44, "")), KeychainState::Empty);
        // 36 is errSecInteractionNotAllowed — the SSH / headless case.
        assert_eq!(
            KeychainState::classify(&exit(36, "")),
            KeychainState::Unavailable
        );
        assert_eq!(
            KeychainState::classify(&exit(1, "")),
            KeychainState::Unavailable
        );
        // Killed by a signal: no exit code at all.
        assert_eq!(
            KeychainState::classify(&KeychainOutput::default()),
            KeychainState::Unavailable
        );
        assert!(exit(0, "").succeeded());
        assert!(!exit(44, "").succeeded());
    }

    #[test]
    fn parse_account_attr_reads_the_acct_blob() {
        let dump = "keychain: \"/Users/x/Library/Keychains/login.keychain-db\"\n    \
                    \"acct\"<blob>=\"somebody\"\n    \"svce\"<blob>=\"Claude Code-credentials\"\n";
        assert_eq!(parse_account_attr(dump).as_deref(), Some("somebody"));
        assert_eq!(parse_account_attr("nothing here"), None);
        // A truncated line must not be mistaken for a value.
        assert_eq!(parse_account_attr("\"acct\"<blob>=\"unterminated"), None);
    }

    #[test]
    fn keychain_read_wins_over_the_fallback_file() {
        let dir = temp_dir();
        let store = composite(FakeKeychain::new(exit(0, "KEYCHAIN\n")), &dir);
        FileStore::new(&dir).write("STALE", "").unwrap();
        assert_eq!(store.read().unwrap().as_deref(), Some("KEYCHAIN"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unreachable_keychain_reads_the_fallback_file() {
        let dir = temp_dir();
        // Exit 36: the SSH session cannot unlock the login keychain, which is
        // exactly when Claude Code is living in the plaintext file.
        let store = composite(FakeKeychain::new(exit(36, "")), &dir);
        FileStore::new(&dir).write("FALLBACK", "").unwrap();
        assert_eq!(store.read().unwrap().as_deref(), Some("FALLBACK"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn empty_keychain_reads_the_fallback_file() {
        let dir = temp_dir();
        let store = composite(FakeKeychain::new(exit(44, "")), &dir);
        FileStore::new(&dir).write("FALLBACK", "").unwrap();
        assert_eq!(store.read().unwrap().as_deref(), Some("FALLBACK"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn read_is_none_when_neither_backend_has_anything() {
        let dir = temp_dir();
        let store = composite(FakeKeychain::new(exit(44, "")), &dir);
        assert_eq!(store.read().unwrap(), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn read_propagates_a_keychain_spawn_failure() {
        let dir = temp_dir();
        let mut fake = FakeKeychain::new(exit(0, ""));
        fake.fail_find = true;
        let store = composite(fake, &dir);
        assert!(store.read().is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn successful_keychain_write_clears_the_stale_fallback_file() {
        let dir = temp_dir();
        let file = FileStore::new(&dir);
        file.write("STALE", "").unwrap();
        let store = composite(FakeKeychain::new(exit(0, "")), &dir);
        store.write("FRESH", "somebody").unwrap();
        // Left in place, this file would resurface as the wrong account the
        // next time the keychain is unreachable.
        assert!(!file.path().exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failed_keychain_write_falls_back_to_the_file_and_clears_the_keychain() {
        let dir = temp_dir();
        let mut fake = FakeKeychain::new(exit(36, ""));
        fake.store_ok = false;
        let store = composite(fake, &dir);
        store.write("FRESH", "somebody").unwrap();
        assert_eq!(
            FileStore::new(&dir).read().unwrap().as_deref(),
            Some("FRESH")
        );
        assert_eq!(store.read().unwrap().as_deref(), Some("FRESH"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn empty_account_attribute_falls_back_to_the_os_username() {
        let dir = temp_dir();
        let fake = FakeKeychain::new(exit(0, ""));
        let log = fake.log();
        // A profile saved before the attribute was captured carries none.
        // Claude Code looks the item up by `-a <username>`, so that is the
        // only attribute under which it would still find the credential.
        composite(fake, &dir).write("FRESH", "").unwrap();
        assert!(last_script(&log).contains("-a \"default-user\""));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_deletes_before_adding_so_no_duplicate_item_is_left() {
        let dir = temp_dir();
        let fake = FakeKeychain::new(exit(0, ""));
        let log = fake.log();
        composite(fake, &dir).write("FRESH", "somebody").unwrap();
        // Adding under a new acct without deleting first would leave a second
        // item behind, and `find-generic-password -s` would pick either one.
        assert_eq!(
            argv(&log),
            [
                format!("delete-generic-password -s {TEST_SERVICE}"),
                "-i".to_string(),
            ]
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_credential_never_reaches_security_argv() {
        let dir = temp_dir();
        let fake = FakeKeychain::new(exit(0, ""));
        let log = fake.log();
        composite(fake, &dir)
            .write("SUPER-SECRET", "somebody")
            .unwrap();

        // `ps` shows argv to every user on the machine, so the blob must not
        // be there — it goes down stdin to `security -i`, hex-encoded.
        assert_eq!(argv(&log).last().unwrap(), "-i");
        assert!(!argv(&log).join(" ").contains("SUPER-SECRET"));
        let script = last_script(&log);
        assert!(!script.contains("SUPER-SECRET"));
        assert!(script.starts_with("add-generic-password -U -a \"somebody\""));
        assert!(script.contains(&format!("-X \"{}\"", hex_encode(b"SUPER-SECRET"))));
        assert!(script.ends_with('\n'));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_oversized_blob_falls_back_to_argv_like_claude_code() {
        let dir = temp_dir();
        let fake = FakeKeychain::new(exit(0, ""));
        let log = fake.log();
        // Hex doubles the blob, so this comfortably exceeds the stdin limit.
        let huge = "x".repeat(STDIN_COMMAND_LIMIT);
        composite(fake, &dir).write(&huge, "somebody").unwrap();

        let calls = log.borrow();
        let (args, stdin) = calls.last().unwrap();
        assert_eq!(args[0], "add-generic-password");
        assert!(stdin.is_none());
        assert_eq!(args[args.len() - 2], "-X");
        drop(calls);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hex_encoding_uses_the_expected_alphabet() {
        assert_eq!(hex_encode(b""), "");
        assert_eq!(hex_encode(b"\x00\x0f\xff"), "000fff");
        assert_eq!(hex_encode("{}".as_bytes()), "7b7d");
    }

    #[test]
    fn the_service_name_is_threaded_through_every_subcommand() {
        let dir = temp_dir();
        let fake = FakeKeychain::new(exit(0, "BLOB"));
        let log = fake.log();
        let store = KeychainStore::new(
            Box::new(fake),
            FileStore::new(&dir),
            "Claude Code-credentials-deadbeef".to_string(),
            "u".to_string(),
        );
        store.read().unwrap();
        store.account_attr().unwrap();
        store.write("BLOB", "u").unwrap();
        // A store scoped to $CLAUDE_CONFIG_DIR must never touch the default
        // item, or a switch inside an isolate swaps the main account.
        for (args, stdin) in log.borrow().iter() {
            let text = format!("{} {}", args.join(" "), stdin.clone().unwrap_or_default());
            assert!(text.contains("Claude Code-credentials-deadbeef"), "{text}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn account_attributes_are_sanitized_like_claude_code() {
        assert_eq!(sanitize_account("mysqto"), "mysqto");
        assert_eq!(sanitize_account("a.b_c-1"), "a.b_c-1");
        // Anything outside Claude Code's allowed set and it substitutes a
        // fixed name — so we must, or we address a different item.
        assert_eq!(sanitize_account(""), FALLBACK_ACCOUNT);
        assert_eq!(sanitize_account("has space"), FALLBACK_ACCOUNT);
        assert_eq!(sanitize_account("quote\"inject"), FALLBACK_ACCOUNT);
        assert_eq!(sanitize_account("ünïcode"), FALLBACK_ACCOUNT);
    }

    #[test]
    fn a_hostile_account_name_cannot_break_out_of_the_stdin_script() {
        let dir = temp_dir();
        let fake = FakeKeychain::new(exit(0, ""));
        let log = fake.log();
        composite(fake, &dir)
            .write("BLOB", "evil\" -s \"Claude Code-credentials")
            .unwrap();
        let script = last_script(&log);
        assert!(script.contains(&format!("-a \"{FALLBACK_ACCOUNT}\"")));
        assert_eq!(script.matches("-s").count(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_propagates_a_keychain_spawn_failure() {
        let dir = temp_dir();
        let mut fake = FakeKeychain::new(exit(0, ""));
        fake.fail_store = true;
        let store = composite(fake, &dir);
        assert!(store.write("FRESH", "somebody").is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn account_attr_comes_from_the_attribute_dump() {
        let dir = temp_dir();
        let mut fake = FakeKeychain::new(exit(0, ""));
        fake.attrs = KeychainOutput {
            code: Some(0),
            stdout: "    \"acct\"<blob>=\"somebody\"\n".to_string(),
            // `-g` puts the secret, not the attributes, on stderr.
            stderr: "password: \"{\\\"token\\\":1}\"\n".to_string(),
        };
        let store = composite(fake, &dir);
        assert_eq!(store.account_attr().unwrap().as_deref(), Some("somebody"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn account_attr_propagates_a_spawn_failure() {
        let dir = temp_dir();
        let mut fake = FakeKeychain::new(exit(0, ""));
        fake.fail_attrs = true;
        let store = composite(fake, &dir);
        assert!(store.account_attr().is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hint_explains_an_unreachable_keychain_only() {
        let dir = temp_dir();
        let store = composite(FakeKeychain::new(exit(36, "")), &dir);
        let hint = store.unavailable_hint().unwrap().unwrap();
        assert!(hint.contains("Keychain is locked or unreachable"));
        assert!(hint.contains(".credentials.json"));

        // A readable-but-empty keychain is a plain "signed out"; no hint.
        let store = composite(FakeKeychain::new(exit(44, "")), &dir);
        assert_eq!(store.unavailable_hint().unwrap(), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hint_propagates_a_spawn_failure() {
        let dir = temp_dir();
        let mut fake = FakeKeychain::new(exit(0, ""));
        fake.fail_find = true;
        let store = composite(fake, &dir);
        assert!(store.unavailable_hint().is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn file_store_has_no_hint() {
        assert_eq!(FileStore::new("/x").unavailable_hint().unwrap(), None);
    }

    #[test]
    fn remove_is_idempotent_and_reports_real_failures() {
        let dir = temp_dir();
        let store = FileStore::new(&dir);
        // Absent is success.
        store.remove().unwrap();
        store.write("blob", "").unwrap();
        store.remove().unwrap();
        assert!(!store.path().exists());
        // A directory at the credential path cannot be unlinked.
        fs::create_dir_all(store.path()).unwrap();
        assert!(store.remove().is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    // ---- backend selection -------------------------------------------------

    #[test]
    fn backend_parses_the_env_var() {
        assert_eq!(Backend::from_env(None), Backend::Auto);
        assert_eq!(Backend::from_env(Some("")), Backend::Auto);
        assert_eq!(Backend::from_env(Some("nonsense")), Backend::Auto);
        assert_eq!(Backend::from_env(Some(" File ")), Backend::File);
        assert_eq!(Backend::from_env(Some("PLAINTEXT")), Backend::File);
        assert_eq!(Backend::default(), Backend::Auto);
    }

    #[test]
    fn select_store_picks_the_composite_only_when_asked_and_able() {
        let dir = temp_dir();
        let file = FileStore::new(&dir);
        file.write("FROM-FILE", "").unwrap();

        // macOS default: the keychain is consulted first.
        let store = select_store(
            Backend::Auto,
            Some(Box::new(FakeKeychain::new(exit(0, "FROM-KEYCHAIN")))),
            dir.clone(),
            TEST_SERVICE.to_string(),
            "u".to_string(),
        );
        assert_eq!(store.read().unwrap().as_deref(), Some("FROM-KEYCHAIN"));

        // Forced to the file even on macOS.
        let store = select_store(
            Backend::File,
            Some(Box::new(FakeKeychain::new(exit(0, "FROM-KEYCHAIN")))),
            dir.clone(),
            TEST_SERVICE.to_string(),
            "u".to_string(),
        );
        assert_eq!(store.read().unwrap().as_deref(), Some("FROM-FILE"));

        // Off macOS there is no keychain to offer.
        let store = select_store(
            Backend::Auto,
            None,
            dir.clone(),
            TEST_SERVICE.to_string(),
            "u".to_string(),
        );
        assert_eq!(store.read().unwrap().as_deref(), Some("FROM-FILE"));
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
