//! Command-line surface: the parsed [`Command`] model and the [`App`] dispatch
//! logic that maps subcommands onto the orchestration layer.
//!
//! Every decision here — the `clap` argument parser, argument validation,
//! reserved-word checks, symlink planning, isolate/seed bookkeeping, search
//! scope resolution, and shell-completion generation — is pure logic exercised
//! by unit tests with a fake [`System`] and temporary directories. The real
//! process-spawning adapter (launching `claude`, `fzf`, `csx`, creating
//! symlinks, prompting, and replacing the process via `exec`) lives behind the
//! [`System`] port in [`crate::cli_shim`], which owns only the real-environment
//! wiring.

use crate::config;
use crate::creds::CredentialStore;
use crate::error::{Error, Result};
use crate::store::{Store, TokenScope};
use crate::switch::Switcher;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use serde_json::Value;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Subcommand words a profile may not be named, to keep the bare
/// `ccswitch <name>` dispatch unambiguous.
pub const RESERVED: &[&str] = &[
    "save", "add", "isolate", "iso", "seed", "search", "s", "list", "ls", "current", "whoami",
    "use", "rm", "remove", "delete", "help",
];

/// Whether `name` collides with a reserved subcommand word.
#[must_use]
pub fn is_reserved(name: &str) -> bool {
    RESERVED.contains(&name)
}

/// The parsed command, decoupled from the `clap` types so all dispatch logic
/// is testable by constructing values directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Sign in a new account and save it as a profile.
    Add {
        /// Profile name.
        name: String,
        /// Overwrite an existing profile of the same name.
        force: bool,
    },
    /// Snapshot the active account into a profile.
    Save {
        /// Profile name.
        name: String,
        /// Overwrite an existing profile of the same name.
        force: bool,
    },
    /// Restore a profile as the active account, without launching `claude`.
    Use {
        /// Profile name.
        name: String,
    },
    /// List saved profiles.
    List,
    /// Show the active account.
    Current,
    /// Delete a saved profile.
    Rm {
        /// Profile name.
        name: String,
    },
    /// Run a concurrent session isolated to a per-profile config dir.
    Isolate {
        /// Isolated profile name; `None` lists existing isolates.
        name: Option<String>,
        /// Extra arguments forwarded to `claude`.
        args: Vec<String>,
    },
    /// Seed the shared isolate memory from `~/.claude` (or `dir`).
    Seed {
        /// Optional source directory.
        dir: Option<String>,
    },
    /// Fuzzy-pick and resume a past session via `csx` + `fzf`.
    Search {
        /// Scope arguments forwarded to `csx sessions`.
        scope: Vec<String>,
    },
    /// Bare `ccswitch <name> [args...]`: switch then launch `claude`.
    Bare {
        /// Profile name.
        name: String,
        /// Extra arguments forwarded to `claude`.
        args: Vec<String>,
    },
    /// Print a shell completion script to stdout.
    Completions {
        /// The shell to generate completions for.
        shell: Shell,
    },
    /// Print usage.
    Help,
}

/// `ccswitch` — switch between multiple Claude Code accounts.
///
/// The `clap` derive front end. Kept here (rather than in the shim) so parsing
/// every subcommand, flag, and alias is exercised by unit tests.
#[derive(Parser, Debug)]
#[command(
    name = "ccswitch",
    version,
    about = "Switch between multiple Claude Code accounts",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// The chosen subcommand, or `None` for bare `ccswitch`.
    #[command(subcommand)]
    pub command: Option<Sub>,
}

/// The `clap`-parsed subcommand, mapped onto the internal [`Command`] model.
#[derive(Subcommand, Debug)]
pub enum Sub {
    /// Sign in a new account and save it as a profile.
    Add {
        /// Profile name.
        name: String,
        /// Overwrite an existing profile of the same name.
        #[arg(long)]
        force: bool,
    },
    /// Snapshot the active account into a profile.
    Save {
        /// Profile name.
        name: String,
        /// Overwrite an existing profile of the same name.
        #[arg(long)]
        force: bool,
    },
    /// Restore a profile as the active account.
    Use {
        /// Profile name.
        name: String,
    },
    /// List saved profiles.
    #[command(visible_alias = "ls")]
    List,
    /// Show the active account.
    #[command(visible_alias = "whoami")]
    Current,
    /// Delete a saved profile.
    #[command(visible_aliases = ["remove", "delete"])]
    Rm {
        /// Profile name.
        name: String,
    },
    /// Run a concurrent session isolated to a per-profile config dir.
    #[command(visible_alias = "iso")]
    Isolate {
        /// Isolated profile name; omit to list existing isolates.
        name: Option<String>,
        /// Extra arguments forwarded to `claude`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Seed the shared isolate memory from `~/.claude` (or a directory).
    Seed {
        /// Optional source directory.
        dir: Option<String>,
    },
    /// Fuzzy-pick and resume a past session via `csx` + `fzf`.
    #[command(visible_alias = "s")]
    Search {
        /// Scope arguments forwarded to `csx sessions`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        scope: Vec<String>,
    },
    /// Print a shell completion script to stdout.
    Completions {
        /// The shell to generate completions for.
        shell: Shell,
    },
    /// Bare `ccswitch <name> [args...]`, or `help`.
    #[command(external_subcommand)]
    External(Vec<String>),
}

impl From<Option<Sub>> for Command {
    fn from(sub: Option<Sub>) -> Self {
        match sub {
            None => Command::Help,
            Some(Sub::Add { name, force }) => Command::Add { name, force },
            Some(Sub::Save { name, force }) => Command::Save { name, force },
            Some(Sub::Use { name }) => Command::Use { name },
            Some(Sub::List) => Command::List,
            Some(Sub::Current) => Command::Current,
            Some(Sub::Rm { name }) => Command::Rm { name },
            Some(Sub::Isolate { name, args }) => Command::Isolate { name, args },
            Some(Sub::Seed { dir }) => Command::Seed { dir },
            Some(Sub::Search { scope }) => Command::Search { scope },
            Some(Sub::Completions { shell }) => Command::Completions { shell },
            Some(Sub::External(mut parts)) => {
                let name = parts.remove(0);
                if name == "help" {
                    Command::Help
                } else {
                    Command::Bare { name, args: parts }
                }
            }
        }
    }
}

/// Parse an argument iterator into the internal [`Command`] model.
///
/// # Errors
///
/// Returns the `clap` error for a parse failure or for the built-in `--help`
/// / `--version` flags, so the caller can render it and exit.
pub fn parse_from<I, T>(args: I) -> std::result::Result<Command, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    Ok(Cli::try_parse_from(args)?.command.into())
}

/// Write a shell completion script for `ccswitch` to `out`.
pub fn generate_completions(shell: Shell, out: &mut dyn Write) {
    let mut cmd = Cli::command();
    generate(shell, &mut cmd, "ccswitch", out);
}

/// The side-effecting operations the CLI needs, behind a port so the decision
/// logic can be tested with a fake. The real adapter is in
/// [`crate::cli_shim`].
pub trait System {
    /// Run `claude auth login`, returning whether it succeeded.
    ///
    /// # Errors
    ///
    /// Returns an error if the process could not be spawned.
    fn claude_login(&self) -> Result<bool>;

    /// Whether `program` is found on `PATH`.
    fn command_exists(&self, program: &str) -> bool;

    /// Whether a `claude` process is currently running.
    fn claude_is_running(&self) -> bool;

    /// Stop the Claude Code daemon and its session workers so the next session
    /// re-reads the restored credentials (`claude daemon stop --any`). A kept
    /// worker would hold its original account in memory and a resumed session
    /// would reattach to it, ignoring the switch — so workers are stopped too.
    /// Best-effort.
    ///
    /// # Errors
    ///
    /// Returns an error if the daemon control command could not be run.
    fn stop_daemon(&self) -> Result<()>;

    /// Create (or replace) a symlink at `link` pointing to `target`.
    ///
    /// # Errors
    ///
    /// Returns an error if the link could not be created.
    fn make_symlink(&self, target: &Path, link: &Path) -> Result<()>;

    /// Prompt the user with `prompt` for a yes/no answer.
    ///
    /// # Errors
    ///
    /// Returns an error if the prompt could not be read.
    fn confirm(&self, prompt: &str) -> Result<bool>;

    /// Run `csx current --json` and return its stdout.
    ///
    /// # Errors
    ///
    /// Returns an error if the process could not be spawned.
    fn csx_current(&self) -> Result<Vec<u8>>;

    /// Run `csx sessions --json <scope...>` and return its stdout.
    ///
    /// # Errors
    ///
    /// Returns an error if the process could not be spawned.
    fn csx_sessions(&self, scope: &[String]) -> Result<Vec<u8>>;

    /// Pipe `rows` into `fzf` (with `preview` as its preview command) and
    /// return the selected line, or `None` when the user cancels.
    ///
    /// # Errors
    ///
    /// Returns an error if `fzf` could not be spawned.
    fn fzf_pick(&self, rows: &str, preview: &str) -> Result<Option<String>>;

    /// Replace the current process with `program args...`, layering `envs` on
    /// top of the inherited environment. On success this never returns.
    ///
    /// # Errors
    ///
    /// Returns an error if the program could not be executed.
    fn exec(&self, program: &str, args: &[String], envs: &[(String, String)]) -> Result<()>;
}

/// Bundled output sinks so every message is captured in tests.
pub struct Io<'a> {
    /// Normal (stdout) output.
    pub out: &'a mut dyn Write,
    /// Diagnostic (stderr) output.
    pub err: &'a mut dyn Write,
}

/// Filesystem locations the CLI operates on, resolved from the environment.
#[derive(Debug, Clone)]
pub struct Paths {
    /// Path to `~/.claude.json`.
    pub config: PathBuf,
    /// Base directory holding isolated profiles.
    pub isolate_base: PathBuf,
    /// Default source directory for `seed` (`~/.claude`).
    pub seed_default: PathBuf,
}

/// Resolve the profile-store root: `$CCSWITCH_HOME` or `~/.claude/accounts`.
#[must_use]
pub fn ccswitch_home(var: Option<String>, home: &Path) -> PathBuf {
    non_empty(var).map_or_else(|| home.join(".claude").join("accounts"), PathBuf::from)
}

/// Resolve the isolate base: `$CCSWITCH_ISOLATE_HOME` or `~/.claude/profiles`.
#[must_use]
pub fn isolate_home(var: Option<String>, home: &Path) -> PathBuf {
    non_empty(var).map_or_else(|| home.join(".claude").join("profiles"), PathBuf::from)
}

/// The `~/.claude.json` config path for a home directory.
#[must_use]
pub fn config_path(home: &Path) -> PathBuf {
    home.join(".claude.json")
}

/// The `~/.claude` directory for a home directory.
#[must_use]
pub fn claude_dir(home: &Path) -> PathBuf {
    home.join(".claude")
}

/// Treat an empty string the same as an unset variable.
fn non_empty(var: Option<String>) -> Option<String> {
    var.filter(|s| !s.is_empty())
}

/// What to do with a would-be symlink at a given path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkPlan {
    /// The path is free; create the link.
    Create,
    /// The path is already a symlink; replace it.
    Replace,
    /// The path exists as a non-symlink; leave it and warn.
    SkipExists,
}

/// Decide how to (re)create the symlink at `link`.
#[must_use]
pub fn plan_link(link: &Path) -> LinkPlan {
    if link.is_symlink() {
        LinkPlan::Replace
    } else if link.exists() {
        LinkPlan::SkipExists
    } else {
        LinkPlan::Create
    }
}

/// Whether the shared isolate directory holds any seeded memory or history.
#[must_use]
pub fn shared_is_seeded(shared: &Path) -> bool {
    if file_is_non_empty(&shared.join("history.jsonl"))
        || file_is_non_empty(&shared.join("CLAUDE.md"))
    {
        return true;
    }
    dir_has_entry(&shared.join("projects"))
}

/// Whether `path` is a file with a non-zero length.
fn file_is_non_empty(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() > 0)
}

/// Whether `dir` exists and contains at least one entry.
fn dir_has_entry(dir: &Path) -> bool {
    fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some())
}

/// The `--tool <value>` override embedded in a search scope, if any.
#[must_use]
pub fn tool_flag_value(scope: &[String]) -> Option<String> {
    let mut it = scope.iter();
    while let Some(arg) = it.next() {
        if arg == "--tool" {
            return it.next().cloned();
        }
    }
    None
}

/// Parse `csx sessions --json` output into `id\tlabel` rows for `fzf`.
///
/// # Errors
///
/// Returns an error if the bytes are not a JSON array.
pub fn parse_session_rows(json: &[u8]) -> Result<Vec<String>> {
    let sessions: Vec<Value> = serde_json::from_slice(json)?;
    Ok(sessions
        .iter()
        .map(|s| {
            let field = |k: &str| s.get(k).and_then(Value::as_str).unwrap_or("-");
            let id = s.get("session_id").and_then(Value::as_str).unwrap_or("");
            let msgs = s
                .get("msg_count")
                .and_then(Value::as_u64)
                .map_or_else(|| "0".to_string(), |n| n.to_string());
            format!(
                "{id}\t{}  {}  {}  ({msgs} msgs)",
                field("tool"),
                field("project_name"),
                field("git_branch"),
            )
        })
        .collect())
}

/// The tool of the first entry in `csx current --json` output, if present.
#[must_use]
pub fn parse_current_tool(json: &[u8]) -> Option<String> {
    let sessions: Vec<Value> = serde_json::from_slice(json).ok()?;
    sessions
        .first()?
        .get("tool")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

/// Orchestrates the subcommands over the credential store, profile store, and
/// [`System`] port.
pub struct App<'a> {
    system: &'a dyn System,
    creds: &'a dyn CredentialStore,
    store: &'a Store,
    paths: Paths,
    scope: TokenScope,
}

impl<'a> App<'a> {
    /// Build an app over its ports and resolved paths.
    pub fn new(
        system: &'a dyn System,
        creds: &'a dyn CredentialStore,
        store: &'a Store,
        paths: Paths,
        scope: TokenScope,
    ) -> Self {
        Self {
            system,
            creds,
            store,
            paths,
            scope,
        }
    }

    fn switcher(&self) -> Switcher<'_> {
        Switcher::new(
            self.creds,
            self.store,
            self.paths.config.clone(),
            self.scope,
        )
    }

    /// Route a parsed command to its handler.
    ///
    /// # Errors
    ///
    /// Propagates the handler's error.
    pub fn dispatch(&self, io: &mut Io<'_>, command: Command) -> Result<()> {
        match command {
            Command::Add { name, force } => self.add(io, &name, force),
            Command::Save { name, force } => self.save(io, &name, force),
            Command::Use { name } => self.use_profile(io, &name),
            Command::List => self.list(io),
            Command::Current => self.current(io),
            Command::Rm { name } => self.rm(io, &name),
            Command::Isolate { name, args } => self.isolate(io, name.as_deref(), &args),
            Command::Seed { dir } => self.seed(io, dir.as_deref()),
            Command::Search { scope } => self.search(io, &scope),
            Command::Bare { name, args } => self.bare(io, &name, &args),
            Command::Completions { shell } => self.completions(io, shell),
            Command::Help => self.help(io),
        }
    }

    /// Print a shell completion script to stdout.
    fn completions(&self, io: &mut Io<'_>, shell: Shell) -> Result<()> {
        generate_completions(shell, io.out);
        Ok(())
    }

    /// Sign in a new account and save it as `name`.
    fn add(&self, io: &mut Io<'_>, name: &str, force: bool) -> Result<()> {
        self.reject_reserved(name)?;
        if self.store.contains(name) && !force {
            return Err(Error::Invalid(format!(
                "profile '{name}' already exists (rm it first, or pass --force)"
            )));
        }
        writeln!(
            io.out,
            "signing in as '{name}' (a browser window will open)..."
        )?;
        if !self.system.claude_login()? {
            return Err(Error::Invalid(
                "login did not complete — nothing saved".to_string(),
            ));
        }
        self.save(io, name, force)
    }

    /// Snapshot the active account into `name`.
    fn save(&self, io: &mut Io<'_>, name: &str, force: bool) -> Result<()> {
        self.reject_reserved(name)?;
        let account = self.switcher().save(name, force)?;
        writeln!(io.out, "saved profile '{name}' ({})", email_of(&account))?;
        Ok(())
    }

    /// Restore `name` as the active account.
    fn use_profile(&self, io: &mut Io<'_>, name: &str) -> Result<()> {
        if self.system.claude_is_running() {
            writeln!(
                io.err,
                "warning: a running 'claude' may overwrite ~/.claude.json on exit; quit it first"
            )?;
        }
        // A running daemon (and its session workers) cache the account's auth;
        // stop them so the next session — including a `claude --resume` — picks
        // up the restored credentials. Best-effort — never block the switch.
        if let Err(e) = self.system.stop_daemon() {
            writeln!(io.err, "warning: could not stop the claude daemon: {e}")?;
        }
        let account = self.switcher().activate(name)?;
        writeln!(io.out, "switched to '{name}' ({})", email_of(&account))?;
        Ok(())
    }

    /// List saved profiles, marking the active one.
    fn list(&self, io: &mut Io<'_>) -> Result<()> {
        let active_key = config::load(&self.paths.config)
            .ok()
            .map(|c| config::account_of(&c).key());
        let profiles = self.store.list()?;
        if profiles.is_empty() {
            writeln!(
                io.out,
                "no profiles yet — save one with 'ccswitch save <name>'"
            )?;
            return Ok(());
        }
        for summary in profiles {
            let account = &summary.account;
            let key = account.key();
            let mark = if key != "|" && Some(&key) == active_key.as_ref() {
                "* "
            } else {
                "  "
            };
            let email = email_of(account);
            if account.org.is_empty() {
                writeln!(io.out, "{mark}{:<16} {email}", summary.name)?;
            } else {
                writeln!(
                    io.out,
                    "{mark}{:<16} {email} ({})",
                    summary.name, account.org
                )?;
            }
        }
        Ok(())
    }

    /// Show the active account.
    fn current(&self, io: &mut Io<'_>) -> Result<()> {
        let config = config::load(&self.paths.config)
            .map_err(|_| Error::Invalid(format!("{} not found", self.paths.config.display())))?;
        let account = config::account_of(&config);
        if account.org.is_empty() {
            writeln!(io.out, "{}", email_of(&account))?;
        } else {
            writeln!(io.out, "{} ({})", email_of(&account), account.org)?;
        }
        Ok(())
    }

    /// Delete a saved profile.
    fn rm(&self, io: &mut Io<'_>, name: &str) -> Result<()> {
        self.store.remove(name)?;
        writeln!(io.out, "removed profile '{name}'")?;
        Ok(())
    }

    /// Switch to `name`, then replace the process with `claude args...`.
    fn bare(&self, io: &mut Io<'_>, name: &str, args: &[String]) -> Result<()> {
        self.use_profile(io, name)?;
        self.system.exec("claude", args, &[])
    }

    /// Run a concurrent session isolated to `name`'s config dir, sharing memory
    /// through symlinks into the shared directory.
    fn isolate(&self, io: &mut Io<'_>, name: Option<&str>, args: &[String]) -> Result<()> {
        let base = &self.paths.isolate_base;
        let shared = base.join("shared");
        let Some(name) = name else {
            return self.list_isolates(io, base);
        };
        if name == "shared" || is_reserved(name) {
            return Err(Error::ReservedName(name.to_string()));
        }

        let dir = base.join(name);
        fs::create_dir_all(shared.join("projects"))?;
        fs::create_dir_all(&dir)?;
        touch(&shared.join("history.jsonl"))?;
        touch(&shared.join("CLAUDE.md"))?;

        for entry in ["projects", "history.jsonl", "CLAUDE.md"] {
            self.link_into(io, &shared.join(entry), &dir.join(entry))?;
        }

        if !shared_is_seeded(&shared) {
            writeln!(
                io.err,
                "warning: shared isolate memory is empty — this session starts with no"
            )?;
            writeln!(
                io.err,
                "  history or CLAUDE.md. Run 'ccswitch seed' first to import your ~/.claude memory."
            )?;
            if !self
                .system
                .confirm(&format!("  launch '{name}' anyway? [y/N] "))?
            {
                return Err(Error::Invalid("aborted — nothing launched".to_string()));
            }
        }

        writeln!(
            io.out,
            "launching isolated '{name}' — CLAUDE_CONFIG_DIR={} (memory shared via {})",
            dir.display(),
            shared.display()
        )?;
        writeln!(io.out, "(first run for a profile will ask you to sign in)")?;
        let env = vec![(
            "CLAUDE_CONFIG_DIR".to_string(),
            dir.to_string_lossy().into_owned(),
        )];
        self.system.exec("claude", args, &env)
    }

    /// List existing isolated profiles under `base`.
    fn list_isolates(&self, io: &mut Io<'_>, base: &Path) -> Result<()> {
        writeln!(io.out, "isolated profiles in {}:", base.display())?;
        let mut any = false;
        if let Ok(entries) = fs::read_dir(base) {
            let mut names: Vec<String> = entries
                .flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n != "shared" && base.join(n).join("projects").is_symlink())
                .collect();
            names.sort();
            for name in names {
                any = true;
                writeln!(io.out, "  {name}")?;
            }
        }
        if !any {
            writeln!(io.out, "  (none yet)")?;
        }
        writeln!(io.out, "usage: ccswitch isolate <name> [claude args...]")?;
        Ok(())
    }

    /// Apply a [`plan_link`] decision for one shared entry.
    fn link_into(&self, io: &mut Io<'_>, target: &Path, link: &Path) -> Result<()> {
        match plan_link(link) {
            LinkPlan::Create | LinkPlan::Replace => self.system.make_symlink(target, link),
            LinkPlan::SkipExists => {
                writeln!(
                    io.err,
                    "warning: {} exists and is not a symlink — leaving it as-is",
                    link.display()
                )?;
                Ok(())
            }
        }
    }

    /// Copy `CLAUDE.md`, `history.jsonl`, and `projects/` from a source into the
    /// shared isolate directory.
    fn seed(&self, io: &mut Io<'_>, dir: Option<&str>) -> Result<()> {
        let src = dir.map_or_else(|| self.paths.seed_default.clone(), PathBuf::from);
        if !src.is_dir() {
            return Err(Error::Invalid(format!(
                "source '{}' not found",
                src.display()
            )));
        }
        let shared = self.paths.isolate_base.join("shared");
        fs::create_dir_all(shared.join("projects"))?;
        writeln!(
            io.out,
            "seeding shared memory in {} from {} ...",
            shared.display(),
            src.display()
        )?;
        let mut did = false;
        for file in ["CLAUDE.md", "history.jsonl"] {
            let from = src.join(file);
            if from.is_file() {
                fs::copy(&from, shared.join(file))?;
                writeln!(io.out, "  {file}")?;
                did = true;
            }
        }
        let projects = src.join("projects");
        if projects.is_dir() {
            copy_dir_recursive(&projects, &shared.join("projects"))?;
            writeln!(io.out, "  projects/ (transcripts + memory)")?;
            did = true;
        }
        if did {
            writeln!(
                io.out,
                "done — isolate profiles now share this memory/history"
            )?;
        } else {
            writeln!(io.out, "  nothing to seed from {}", src.display())?;
        }
        Ok(())
    }

    /// Fuzzy-pick a past session via `csx` + `fzf` and resume it.
    fn search(&self, io: &mut Io<'_>, scope: &[String]) -> Result<()> {
        if !self.system.command_exists("csx") {
            return Err(Error::Invalid(
                "csx not found on PATH — see github.com/mysqto/csx".to_string(),
            ));
        }
        if !self.system.command_exists("fzf") {
            return Err(Error::Invalid("fzf not found on PATH".to_string()));
        }

        let explicit = tool_flag_value(scope);
        let tool = explicit.clone().or_else(|| self.default_tool());
        let effective: Vec<String> = if explicit.is_some() {
            scope.to_vec()
        } else if let Some(tool) = &tool {
            let mut v = vec!["--tool".to_string(), tool.clone()];
            v.extend_from_slice(scope);
            v
        } else {
            scope.to_vec()
        };

        let rows = parse_session_rows(&self.system.csx_sessions(&effective)?)?;
        if rows.is_empty() {
            return Err(Error::Invalid("no sessions matched".to_string()));
        }
        let Some(picked) = self.system.fzf_pick(&rows.join("\n"), "csx show {1}")? else {
            return Ok(());
        };
        let id = picked.split('\t').next().unwrap_or("").trim();
        if id.is_empty() {
            return Ok(());
        }
        self.resume(io, tool.as_deref(), id)
    }

    /// The active tool reported by `csx current`, ignoring any failure.
    fn default_tool(&self) -> Option<String> {
        parse_current_tool(&self.system.csx_current().ok()?)
    }

    /// Resume session `id` with the command for `tool`.
    fn resume(&self, _io: &mut Io<'_>, tool: Option<&str>, id: &str) -> Result<()> {
        let (program, args) = match tool {
            Some("codex") => ("codex", vec!["resume".to_string(), id.to_string()]),
            None | Some("" | "claude-code") => {
                ("claude", vec!["--resume".to_string(), id.to_string()])
            }
            Some(other) => {
                return Err(Error::Invalid(format!(
                    "don't know how to resume tool '{other}' (session {id})"
                )))
            }
        };
        self.system.exec(program, &args, &[])
    }

    /// Print usage.
    fn help(&self, io: &mut Io<'_>) -> Result<()> {
        write!(io.out, "{}", help_text())?;
        Ok(())
    }

    fn reject_reserved(&self, name: &str) -> Result<()> {
        if is_reserved(name) {
            Err(Error::ReservedName(name.to_string()))
        } else {
            Ok(())
        }
    }
}

/// The email of an account, or `unknown` when absent.
fn email_of(account: &crate::model::Account) -> String {
    if account.email.is_empty() {
        "unknown".to_string()
    } else {
        account.email.clone()
    }
}

/// Create an empty file at `path` when it does not already exist.
fn touch(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::write(path, "")?;
    }
    Ok(())
}

/// Recursively copy the contents of `src` into `dst`.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// The `ccswitch help` usage text.
#[must_use]
pub fn help_text() -> String {
    let lines = [
        "ccswitch — switch between multiple Claude Code accounts",
        "",
        "usage:",
        "  ccswitch <name> [args...]  switch to <name> and start a claude session",
        "  ccswitch use <name>        switch to <name> without launching claude",
        "  ccswitch add <name>        sign in to a new account and save it as <name>",
        "  ccswitch save <name>       save the current account as <name>",
        "  ccswitch isolate <name>    run a concurrent session isolated to <name>",
        "  ccswitch seed [dir]        seed the shared isolate memory from ~/.claude",
        "  ccswitch search [scope]    fuzzy-pick a past session (via csx) and resume it",
        "  ccswitch list | ls         list saved profiles (* marks the active one)",
        "  ccswitch current | whoami  show the active account",
        "  ccswitch rm <name>         delete a saved profile",
        "  ccswitch completions <sh>  print a completion script (bash|zsh|fish|...)",
        "  ccswitch help              show this help",
        "",
        "profiles live in $CCSWITCH_HOME (default ~/.claude/accounts).",
        "isolated profiles live in $CCSWITCH_ISOLATE_HOME (default ~/.claude/profiles).",
    ];
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    // ---- fakes -------------------------------------------------------------

    struct FakeCreds {
        blob: RefCell<Option<String>>,
        acct: Option<String>,
    }

    impl FakeCreds {
        fn with(blob: Option<&str>, acct: Option<&str>) -> Self {
            Self {
                blob: RefCell::new(blob.map(str::to_string)),
                acct: acct.map(str::to_string),
            }
        }
    }

    impl CredentialStore for FakeCreds {
        fn read(&self) -> Result<Option<String>> {
            Ok(self.blob.borrow().clone())
        }
        fn write(&self, blob: &str, _acct: &str) -> Result<()> {
            *self.blob.borrow_mut() = Some(blob.to_string());
            Ok(())
        }
        fn account_attr(&self) -> Result<Option<String>> {
            Ok(self.acct.clone())
        }
    }

    /// A recorded `exec` call: program, args, and extra environment.
    type ExecCall = (String, Vec<String>, Vec<(String, String)>);

    #[derive(Default)]
    struct FakeSystem {
        login_ok: bool,
        login_err: bool,
        running: bool,
        has_csx: bool,
        has_fzf: bool,
        confirm_yes: bool,
        confirm_err: bool,
        current: Option<Vec<u8>>,
        current_err: bool,
        sessions: Vec<u8>,
        sessions_err: bool,
        pick: Option<String>,
        pick_err: bool,
        symlink_err: bool,
        exec_err: bool,
        daemon_stop_err: bool,
        links: RefCell<Vec<(PathBuf, PathBuf)>>,
        execs: RefCell<Vec<ExecCall>>,
        sessions_scope: RefCell<Vec<String>>,
        daemon_stops: RefCell<u32>,
    }

    impl System for FakeSystem {
        fn claude_login(&self) -> Result<bool> {
            if self.login_err {
                return Err(Error::Invalid("spawn".to_string()));
            }
            Ok(self.login_ok)
        }
        fn command_exists(&self, program: &str) -> bool {
            (program == "csx" && self.has_csx) || (program == "fzf" && self.has_fzf)
        }
        fn claude_is_running(&self) -> bool {
            self.running
        }
        fn stop_daemon(&self) -> Result<()> {
            *self.daemon_stops.borrow_mut() += 1;
            if self.daemon_stop_err {
                return Err(Error::Invalid("daemon".to_string()));
            }
            Ok(())
        }
        fn make_symlink(&self, target: &Path, link: &Path) -> Result<()> {
            if self.symlink_err {
                return Err(Error::Invalid("symlink".to_string()));
            }
            self.links
                .borrow_mut()
                .push((target.to_path_buf(), link.to_path_buf()));
            // Actually create it so downstream planning sees a symlink.
            #[cfg(unix)]
            {
                let _ = std::fs::remove_file(link);
                std::os::unix::fs::symlink(target, link)?;
            }
            Ok(())
        }
        fn confirm(&self, _prompt: &str) -> Result<bool> {
            if self.confirm_err {
                return Err(Error::Invalid("confirm".to_string()));
            }
            Ok(self.confirm_yes)
        }
        fn csx_current(&self) -> Result<Vec<u8>> {
            if self.current_err {
                return Err(Error::Invalid("csx".to_string()));
            }
            Ok(self.current.clone().unwrap_or_default())
        }
        fn csx_sessions(&self, scope: &[String]) -> Result<Vec<u8>> {
            *self.sessions_scope.borrow_mut() = scope.to_vec();
            if self.sessions_err {
                return Err(Error::Invalid("csx".to_string()));
            }
            Ok(self.sessions.clone())
        }
        fn fzf_pick(&self, _rows: &str, _preview: &str) -> Result<Option<String>> {
            if self.pick_err {
                return Err(Error::Invalid("fzf".to_string()));
            }
            Ok(self.pick.clone())
        }
        fn exec(&self, program: &str, args: &[String], envs: &[(String, String)]) -> Result<()> {
            if self.exec_err {
                return Err(Error::Invalid("exec".to_string()));
            }
            self.execs
                .borrow_mut()
                .push((program.to_string(), args.to_vec(), envs.to_vec()));
            Ok(())
        }
    }

    // ---- harness -----------------------------------------------------------

    fn temp_dir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "ccswitch-cli-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&base).unwrap();
        base
    }

    fn config_json(acct: &str, org: &str, email: &str, org_name: &str) -> Value {
        json!({
            "numStartups": 2,
            "userID": "uid",
            "oauthAccount": {
                "accountUuid": acct,
                "organizationUuid": org,
                "emailAddress": email,
                "organizationName": org_name
            }
        })
    }

    fn write_config(dir: &Path, value: &Value) -> PathBuf {
        let path = dir.join(".claude.json");
        fs::write(&path, serde_json::to_string_pretty(value).unwrap()).unwrap();
        path
    }

    struct Bufs {
        out: Vec<u8>,
        err: Vec<u8>,
    }

    impl Bufs {
        fn new() -> Self {
            Self {
                out: Vec::new(),
                err: Vec::new(),
            }
        }
        fn io(&mut self) -> Io<'_> {
            Io {
                out: &mut self.out,
                err: &mut self.err,
            }
        }
        fn out(&self) -> String {
            String::from_utf8(self.out.clone()).unwrap()
        }
        fn err(&self) -> String {
            String::from_utf8(self.err.clone()).unwrap()
        }
    }

    fn paths_for(dir: &Path) -> Paths {
        Paths {
            config: dir.join(".claude.json"),
            isolate_base: dir.join("profiles"),
            seed_default: dir.join("home-claude"),
        }
    }

    // ---- pure helpers ------------------------------------------------------

    #[test]
    fn reserved_words_are_rejected() {
        assert!(is_reserved("list"));
        assert!(is_reserved("help"));
        assert!(!is_reserved("dev"));
    }

    #[test]
    fn path_resolution_prefers_env_then_defaults() {
        let home = Path::new("/home/u");
        assert_eq!(
            ccswitch_home(None, home),
            PathBuf::from("/home/u/.claude/accounts")
        );
        assert_eq!(
            ccswitch_home(Some(String::new()), home),
            PathBuf::from("/home/u/.claude/accounts")
        );
        assert_eq!(
            ccswitch_home(Some("/custom".to_string()), home),
            PathBuf::from("/custom")
        );
        assert_eq!(
            isolate_home(None, home),
            PathBuf::from("/home/u/.claude/profiles")
        );
        assert_eq!(
            isolate_home(Some("/iso".to_string()), home),
            PathBuf::from("/iso")
        );
        assert_eq!(config_path(home), PathBuf::from("/home/u/.claude.json"));
        assert_eq!(claude_dir(home), PathBuf::from("/home/u/.claude"));
    }

    #[test]
    fn plan_link_distinguishes_states() {
        let dir = temp_dir();
        assert_eq!(plan_link(&dir.join("free")), LinkPlan::Create);
        let file = dir.join("file");
        fs::write(&file, "x").unwrap();
        assert_eq!(plan_link(&file), LinkPlan::SkipExists);
        #[cfg(unix)]
        {
            let link = dir.join("link");
            std::os::unix::fs::symlink(&file, &link).unwrap();
            assert_eq!(plan_link(&link), LinkPlan::Replace);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn shared_is_seeded_detects_each_source() {
        let dir = temp_dir();
        assert!(!shared_is_seeded(&dir));
        // Empty files/dirs do not count.
        fs::write(dir.join("history.jsonl"), "").unwrap();
        fs::write(dir.join("CLAUDE.md"), "").unwrap();
        fs::create_dir_all(dir.join("projects")).unwrap();
        assert!(!shared_is_seeded(&dir));
        // A non-empty history counts.
        fs::write(dir.join("history.jsonl"), "line").unwrap();
        assert!(shared_is_seeded(&dir));

        let dir2 = temp_dir();
        fs::write(dir2.join("CLAUDE.md"), "notes").unwrap();
        assert!(shared_is_seeded(&dir2));

        let dir3 = temp_dir();
        fs::create_dir_all(dir3.join("projects").join("p")).unwrap();
        assert!(shared_is_seeded(&dir3));

        fs::remove_dir_all(&dir).unwrap();
        fs::remove_dir_all(&dir2).unwrap();
        fs::remove_dir_all(&dir3).unwrap();
    }

    #[test]
    fn tool_flag_value_reads_override() {
        assert_eq!(
            tool_flag_value(&["--tool".to_string(), "codex".to_string()]),
            Some("codex".to_string())
        );
        assert_eq!(tool_flag_value(&["--tool".to_string()]), None);
        assert_eq!(tool_flag_value(&["--foo".to_string()]), None);
    }

    #[test]
    fn parse_session_rows_projects_fields() {
        let json = br#"[
            {"session_id":"s1","tool":"claude-code","project_name":"proj","git_branch":"main","msg_count":5},
            {"session_id":"s2"}
        ]"#;
        let rows = parse_session_rows(json).unwrap();
        assert_eq!(rows[0], "s1\tclaude-code  proj  main  (5 msgs)");
        assert_eq!(rows[1], "s2\t-  -  -  (0 msgs)");
    }

    #[test]
    fn parse_session_rows_rejects_non_array() {
        assert!(parse_session_rows(b"not json").is_err());
    }

    #[test]
    fn parse_current_tool_reads_first_entry() {
        assert_eq!(
            parse_current_tool(br#"[{"tool":"codex"}]"#),
            Some("codex".to_string())
        );
        assert_eq!(parse_current_tool(br#"[{"tool":""}]"#), None);
        assert_eq!(parse_current_tool(b"[]"), None);
        assert_eq!(parse_current_tool(b"nope"), None);
        assert_eq!(parse_current_tool(br#"[{}]"#), None);
    }

    #[test]
    fn help_text_lists_commands() {
        let text = help_text();
        assert!(text.contains("ccswitch <name>"));
        assert!(text.contains("completions"));
        assert!(text.ends_with('\n'));
    }

    // ---- clap parsing ------------------------------------------------------

    /// Parse `args` (without the leading binary name) into a [`Command`].
    fn parse(args: &[&str]) -> Command {
        let mut argv = vec!["ccswitch"];
        argv.extend_from_slice(args);
        parse_from(argv).unwrap()
    }

    #[test]
    fn parse_no_args_is_help() {
        assert_eq!(parse(&[]), Command::Help);
    }

    #[test]
    fn parse_add_with_and_without_force() {
        assert_eq!(
            parse(&["add", "dev"]),
            Command::Add {
                name: "dev".to_string(),
                force: false
            }
        );
        assert_eq!(
            parse(&["add", "dev", "--force"]),
            Command::Add {
                name: "dev".to_string(),
                force: true
            }
        );
    }

    #[test]
    fn parse_save_with_and_without_force() {
        assert_eq!(
            parse(&["save", "dev"]),
            Command::Save {
                name: "dev".to_string(),
                force: false
            }
        );
        assert_eq!(
            parse(&["save", "dev", "--force"]),
            Command::Save {
                name: "dev".to_string(),
                force: true
            }
        );
    }

    #[test]
    fn parse_use() {
        assert_eq!(
            parse(&["use", "dev"]),
            Command::Use {
                name: "dev".to_string()
            }
        );
    }

    #[test]
    fn parse_list_and_ls_alias() {
        assert_eq!(parse(&["list"]), Command::List);
        assert_eq!(parse(&["ls"]), Command::List);
    }

    #[test]
    fn parse_current_and_whoami_alias() {
        assert_eq!(parse(&["current"]), Command::Current);
        assert_eq!(parse(&["whoami"]), Command::Current);
    }

    #[test]
    fn parse_rm_and_aliases() {
        for word in ["rm", "remove", "delete"] {
            assert_eq!(
                parse(&[word, "dev"]),
                Command::Rm {
                    name: "dev".to_string()
                }
            );
        }
    }

    #[test]
    fn parse_isolate_with_name_args_and_alias() {
        assert_eq!(
            parse(&["isolate", "work", "--verbose", "-p"]),
            Command::Isolate {
                name: Some("work".to_string()),
                args: vec!["--verbose".to_string(), "-p".to_string()],
            }
        );
        assert_eq!(
            parse(&["iso"]),
            Command::Isolate {
                name: None,
                args: vec![],
            }
        );
    }

    #[test]
    fn parse_seed_with_and_without_dir() {
        assert_eq!(parse(&["seed"]), Command::Seed { dir: None });
        assert_eq!(
            parse(&["seed", "/src"]),
            Command::Seed {
                dir: Some("/src".to_string())
            }
        );
    }

    #[test]
    fn parse_search_and_alias_with_scope() {
        assert_eq!(
            parse(&["search", "--tool", "codex"]),
            Command::Search {
                scope: vec!["--tool".to_string(), "codex".to_string()],
            }
        );
        assert_eq!(parse(&["s"]), Command::Search { scope: vec![] });
    }

    #[test]
    fn parse_completions_for_each_shell() {
        let cases = [
            ("bash", Shell::Bash),
            ("zsh", Shell::Zsh),
            ("fish", Shell::Fish),
            ("powershell", Shell::PowerShell),
            ("elvish", Shell::Elvish),
        ];
        for (word, shell) in cases {
            assert_eq!(
                parse(&["completions", word]),
                Command::Completions { shell }
            );
        }
    }

    #[test]
    fn parse_bare_name_and_args() {
        assert_eq!(
            parse(&["prod"]),
            Command::Bare {
                name: "prod".to_string(),
                args: vec![],
            }
        );
        assert_eq!(
            parse(&["prod", "--resume", "abc"]),
            Command::Bare {
                name: "prod".to_string(),
                args: vec!["--resume".to_string(), "abc".to_string()],
            }
        );
    }

    #[test]
    fn parse_help_word_is_help() {
        assert_eq!(parse(&["help"]), Command::Help);
    }

    #[test]
    fn parse_help_and_version_flags_error() {
        assert_eq!(
            parse_from(["ccswitch", "--help"]).unwrap_err().kind(),
            clap::error::ErrorKind::DisplayHelp
        );
        assert_eq!(
            parse_from(["ccswitch", "--version"]).unwrap_err().kind(),
            clap::error::ErrorKind::DisplayVersion
        );
    }

    #[test]
    fn parse_unknown_flag_before_subcommand_errors() {
        assert!(parse_from(["ccswitch", "--nope"]).is_err());
    }

    // ---- completion generation ---------------------------------------------

    #[test]
    fn generate_completions_emits_shell_appropriate_scripts() {
        let cases: [(Shell, &str); 5] = [
            (Shell::Bash, "complete -F"),
            (Shell::Zsh, "#compdef ccswitch"),
            (Shell::Fish, "complete -c ccswitch"),
            (Shell::PowerShell, "Register-ArgumentCompleter"),
            (Shell::Elvish, "edit:completion"),
        ];
        for (shell, marker) in cases {
            let mut buf: Vec<u8> = Vec::new();
            generate_completions(shell, &mut buf);
            let script = String::from_utf8(buf).unwrap();
            assert!(!script.is_empty(), "{shell:?} produced no output");
            assert!(
                script.contains(marker),
                "{shell:?} script missing {marker:?}:\n{script}"
            );
            assert!(script.contains("ccswitch"));
        }
    }

    #[test]
    fn dispatch_completions_writes_script_to_stdout() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(&mut bufs.io(), Command::Completions { shell: Shell::Fish })
            .unwrap();
        assert!(bufs.out().contains("complete -c ccswitch"));
        fs::remove_dir_all(&dir).unwrap();
    }

    // ---- add ---------------------------------------------------------------

    fn app_env<'a>(
        system: &'a FakeSystem,
        creds: &'a FakeCreds,
        store: &'a Store,
        dir: &Path,
    ) -> App<'a> {
        App::new(system, creds, store, paths_for(dir), TokenScope::PerAccount)
    }

    #[test]
    fn add_logs_in_then_saves() {
        let dir = temp_dir();
        write_config(&dir, &config_json("A", "O", "a@example.com", "Org A"));
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(Some("BLOB"), Some("attr"));
        let system = FakeSystem {
            login_ok: true,
            ..Default::default()
        };
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(
            &mut bufs.io(),
            Command::Add {
                name: "dev".to_string(),
                force: false,
            },
        )
        .unwrap();
        assert!(bufs.out().contains("signing in as 'dev'"));
        assert!(bufs.out().contains("saved profile 'dev' (a@example.com)"));
        assert!(store.contains("dev"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn add_reserved_name_errors() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(Some("BLOB"), None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        assert!(matches!(
            app.dispatch(
                &mut bufs.io(),
                Command::Add {
                    name: "list".to_string(),
                    force: false
                }
            ),
            Err(Error::ReservedName(_))
        ));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn add_existing_without_force_errors_before_login() {
        let dir = temp_dir();
        write_config(&dir, &config_json("A", "O", "a@x", "Org"));
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(Some("BLOB"), None);
        let system = FakeSystem {
            login_ok: true,
            ..Default::default()
        };
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(
            &mut bufs.io(),
            Command::Add {
                name: "dev".to_string(),
                force: false,
            },
        )
        .unwrap();
        // Second add without force fails and does not attempt login.
        let err = app
            .dispatch(
                &mut bufs.io(),
                Command::Add {
                    name: "dev".to_string(),
                    force: false,
                },
            )
            .unwrap_err();
        assert!(matches!(err, Error::Invalid(m) if m.contains("already exists")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn add_force_overwrites_existing() {
        let dir = temp_dir();
        write_config(&dir, &config_json("A", "O", "a@x", "Org"));
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(Some("BLOB"), None);
        let system = FakeSystem {
            login_ok: true,
            ..Default::default()
        };
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.save(&mut bufs.io(), "dev", false).unwrap();
        *creds.blob.borrow_mut() = Some("BLOB2".to_string());
        app.dispatch(
            &mut bufs.io(),
            Command::Add {
                name: "dev".to_string(),
                force: true,
            },
        )
        .unwrap();
        assert_eq!(store.load_credentials("dev").unwrap(), "BLOB2");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn add_login_failure_saves_nothing() {
        let dir = temp_dir();
        write_config(&dir, &config_json("A", "O", "a@x", "Org"));
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(Some("BLOB"), None);
        let system = FakeSystem {
            login_ok: false,
            ..Default::default()
        };
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        let err = app
            .dispatch(
                &mut bufs.io(),
                Command::Add {
                    name: "dev".to_string(),
                    force: false,
                },
            )
            .unwrap_err();
        assert!(matches!(err, Error::Invalid(m) if m.contains("login did not complete")));
        assert!(!store.contains("dev"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn add_login_spawn_error_propagates() {
        let dir = temp_dir();
        write_config(&dir, &config_json("A", "O", "a@x", "Org"));
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(Some("BLOB"), None);
        let system = FakeSystem {
            login_err: true,
            ..Default::default()
        };
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        assert!(app
            .dispatch(
                &mut bufs.io(),
                Command::Add {
                    name: "dev".to_string(),
                    force: false
                }
            )
            .is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    // ---- save --------------------------------------------------------------

    #[test]
    fn save_reserved_name_errors() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(Some("BLOB"), None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        assert!(matches!(
            app.dispatch(
                &mut bufs.io(),
                Command::Save {
                    name: "use".to_string(),
                    force: false
                }
            ),
            Err(Error::ReservedName(_))
        ));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_unknown_email_prints_unknown() {
        let dir = temp_dir();
        write_config(
            &dir,
            &json!({ "oauthAccount": { "accountUuid": "A" }, "userID": "u" }),
        );
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(Some("BLOB"), None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(
            &mut bufs.io(),
            Command::Save {
                name: "dev".to_string(),
                force: false,
            },
        )
        .unwrap();
        assert!(bufs.out().contains("saved profile 'dev' (unknown)"));
        fs::remove_dir_all(&dir).unwrap();
    }

    // ---- use / bare --------------------------------------------------------

    fn seed_profile(store: &Store, name: &str, acct: &str, org: &str, email: &str) {
        store
            .save_profile(
                name,
                "TOKEN",
                &json!({
                    "oauthAccount": {
                        "accountUuid": acct,
                        "organizationUuid": org,
                        "emailAddress": email,
                        "organizationName": "Org"
                    },
                    "userID": "uid",
                    "keychain_account": "attr"
                }),
            )
            .unwrap();
    }

    #[test]
    fn use_switches_and_warns_when_running() {
        let dir = temp_dir();
        write_config(&dir, &config_json("B", "OB", "b@x", "Org B"));
        let store = Store::new(dir.join("accounts"));
        seed_profile(&store, "dev", "A", "OA", "a@example.com");
        let creds = FakeCreds::with(Some("LIVE"), Some("attr"));
        let system = FakeSystem {
            running: true,
            ..Default::default()
        };
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(
            &mut bufs.io(),
            Command::Use {
                name: "dev".to_string(),
            },
        )
        .unwrap();
        assert!(bufs.err().contains("warning: a running 'claude'"));
        assert!(bufs.out().contains("switched to 'dev' (a@example.com)"));
        // The daemon supervisor is stopped as part of the switch.
        assert_eq!(*system.daemon_stops.borrow(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn use_warns_but_still_switches_when_daemon_stop_fails() {
        let dir = temp_dir();
        write_config(&dir, &config_json("B", "OB", "b@x", "Org B"));
        let store = Store::new(dir.join("accounts"));
        seed_profile(&store, "dev", "A", "OA", "a@example.com");
        let creds = FakeCreds::with(Some("LIVE"), Some("attr"));
        let system = FakeSystem {
            daemon_stop_err: true,
            ..Default::default()
        };
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(
            &mut bufs.io(),
            Command::Use {
                name: "dev".to_string(),
            },
        )
        .unwrap();
        assert!(bufs.err().contains("could not stop the claude daemon"));
        assert!(bufs.out().contains("switched to 'dev' (a@example.com)"));
        assert_eq!(*system.daemon_stops.borrow(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn use_missing_profile_errors() {
        let dir = temp_dir();
        write_config(&dir, &config_json("B", "OB", "b@x", "Org B"));
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(Some("LIVE"), None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        assert!(matches!(
            app.dispatch(
                &mut bufs.io(),
                Command::Use {
                    name: "ghost".to_string()
                }
            ),
            Err(Error::ProfileNotFound(_))
        ));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bare_switches_then_execs_claude() {
        let dir = temp_dir();
        write_config(&dir, &config_json("B", "OB", "b@x", "Org B"));
        let store = Store::new(dir.join("accounts"));
        seed_profile(&store, "dev", "A", "OA", "a@x");
        let creds = FakeCreds::with(Some("LIVE"), Some("attr"));
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(
            &mut bufs.io(),
            Command::Bare {
                name: "dev".to_string(),
                args: vec!["--resume".to_string()],
            },
        )
        .unwrap();
        let execs = system.execs.borrow();
        assert_eq!(execs[0].0, "claude");
        assert_eq!(execs[0].1, vec!["--resume".to_string()]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bare_does_not_exec_when_switch_fails() {
        let dir = temp_dir();
        write_config(&dir, &config_json("B", "OB", "b@x", "Org B"));
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(Some("LIVE"), None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        assert!(app
            .dispatch(
                &mut bufs.io(),
                Command::Bare {
                    name: "ghost".to_string(),
                    args: vec![]
                }
            )
            .is_err());
        assert!(system.execs.borrow().is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    // ---- list / current / rm ----------------------------------------------

    #[test]
    fn list_marks_active_and_shows_orgs() {
        let dir = temp_dir();
        write_config(&dir, &config_json("A", "OA", "a@x", "Org A"));
        let store = Store::new(dir.join("accounts"));
        seed_profile(&store, "dev", "A", "OA", "a@example.com");
        // A profile with no org name to hit the no-org branch.
        store
            .save_profile(
                "plain",
                "T",
                &json!({ "oauthAccount": { "accountUuid": "Z", "emailAddress": "z@x" }, "userID": "u" }),
            )
            .unwrap();
        let creds = FakeCreds::with(Some("LIVE"), None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(&mut bufs.io(), Command::List).unwrap();
        let out = bufs.out();
        assert!(out.contains("* dev"));
        assert!(out.contains("a@example.com (Org)"));
        assert!(out.contains("  plain"));
        assert!(out.contains("z@x"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn list_without_config_marks_nothing() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        seed_profile(&store, "dev", "A", "OA", "a@x");
        let creds = FakeCreds::with(Some("LIVE"), None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(&mut bufs.io(), Command::List).unwrap();
        assert!(!bufs.out().contains("* dev"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn list_empty_store_hints() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(Some("LIVE"), None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(&mut bufs.io(), Command::List).unwrap();
        assert!(bufs.out().contains("no profiles yet"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn list_propagates_store_error() {
        let dir = temp_dir();
        // Root is a file → list errors.
        let file_root = dir.join("file");
        fs::write(&file_root, "x").unwrap();
        let store = Store::new(&file_root);
        let creds = FakeCreds::with(Some("LIVE"), None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        assert!(app.dispatch(&mut bufs.io(), Command::List).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn current_shows_email_and_org() {
        let dir = temp_dir();
        write_config(&dir, &config_json("A", "OA", "a@example.com", "Org A"));
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(Some("LIVE"), None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(&mut bufs.io(), Command::Current).unwrap();
        assert_eq!(bufs.out(), "a@example.com (Org A)\n");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn current_without_org_shows_email_only() {
        let dir = temp_dir();
        write_config(&dir, &json!({ "oauthAccount": { "emailAddress": "a@x" } }));
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(Some("LIVE"), None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(&mut bufs.io(), Command::Current).unwrap();
        assert_eq!(bufs.out(), "a@x\n");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn current_without_config_errors() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(Some("LIVE"), None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        assert!(matches!(
            app.dispatch(&mut bufs.io(), Command::Current),
            Err(Error::Invalid(m)) if m.contains("not found")
        ));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rm_removes_and_reports() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        seed_profile(&store, "dev", "A", "OA", "a@x");
        let creds = FakeCreds::with(Some("LIVE"), None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(
            &mut bufs.io(),
            Command::Rm {
                name: "dev".to_string(),
            },
        )
        .unwrap();
        assert!(bufs.out().contains("removed profile 'dev'"));
        assert!(!store.contains("dev"));
        assert!(matches!(
            app.dispatch(
                &mut bufs.io(),
                Command::Rm {
                    name: "dev".to_string()
                }
            ),
            Err(Error::ProfileNotFound(_))
        ));
        fs::remove_dir_all(&dir).unwrap();
    }

    // ---- help --------------------------------------------------------------

    #[test]
    fn help_prints_usage() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(&mut bufs.io(), Command::Help).unwrap();
        assert!(bufs
            .out()
            .contains("switch between multiple Claude Code accounts"));
        fs::remove_dir_all(&dir).unwrap();
    }

    // ---- isolate -----------------------------------------------------------

    fn seed_shared(base: &Path) {
        let shared = base.join("shared");
        fs::create_dir_all(shared.join("projects")).unwrap();
        fs::write(shared.join("history.jsonl"), "seeded").unwrap();
    }

    #[test]
    fn isolate_links_and_execs_when_seeded() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let base = dir.join("profiles");
        seed_shared(&base);
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(
            &mut bufs.io(),
            Command::Isolate {
                name: Some("work".to_string()),
                args: vec!["--verbose".to_string()],
            },
        )
        .unwrap();
        assert!(bufs.out().contains("launching isolated 'work'"));
        // Three symlinks created; claude exec'd with the config-dir env.
        assert_eq!(system.links.borrow().len(), 3);
        let execs = system.execs.borrow();
        assert_eq!(execs[0].0, "claude");
        assert_eq!(execs[0].1, vec!["--verbose".to_string()]);
        assert_eq!(execs[0].2[0].0, "CLAUDE_CONFIG_DIR");
        assert!(execs[0].2[0].1.ends_with("work"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn isolate_unseeded_confirm_yes_launches() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem {
            confirm_yes: true,
            ..Default::default()
        };
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(
            &mut bufs.io(),
            Command::Isolate {
                name: Some("work".to_string()),
                args: vec![],
            },
        )
        .unwrap();
        assert!(bufs.err().contains("shared isolate memory is empty"));
        assert_eq!(system.execs.borrow().len(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn isolate_unseeded_confirm_no_aborts() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem {
            confirm_yes: false,
            ..Default::default()
        };
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        let err = app
            .dispatch(
                &mut bufs.io(),
                Command::Isolate {
                    name: Some("work".to_string()),
                    args: vec![],
                },
            )
            .unwrap_err();
        assert!(matches!(err, Error::Invalid(m) if m.contains("aborted")));
        assert!(system.execs.borrow().is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn isolate_confirm_error_propagates() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem {
            confirm_err: true,
            ..Default::default()
        };
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        assert!(app
            .dispatch(
                &mut bufs.io(),
                Command::Isolate {
                    name: Some("work".to_string()),
                    args: vec![]
                }
            )
            .is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn isolate_reserved_name_errors() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        for name in ["shared", "list"] {
            assert!(matches!(
                app.dispatch(
                    &mut bufs.io(),
                    Command::Isolate {
                        name: Some(name.to_string()),
                        args: vec![]
                    }
                ),
                Err(Error::ReservedName(_))
            ));
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn isolate_symlink_error_propagates() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let base = dir.join("profiles");
        seed_shared(&base);
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem {
            symlink_err: true,
            ..Default::default()
        };
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        assert!(app
            .dispatch(
                &mut bufs.io(),
                Command::Isolate {
                    name: Some("work".to_string()),
                    args: vec![]
                }
            )
            .is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn isolate_skips_existing_non_symlink_entry() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let base = dir.join("profiles");
        seed_shared(&base);
        // Pre-create a real (non-symlink) projects dir in the profile.
        let profile = base.join("work");
        fs::create_dir_all(profile.join("projects")).unwrap();
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(
            &mut bufs.io(),
            Command::Isolate {
                name: Some("work".to_string()),
                args: vec![],
            },
        )
        .unwrap();
        assert!(bufs.err().contains("is not a symlink"));
        // Only the two non-conflicting entries were linked.
        assert_eq!(system.links.borrow().len(), 2);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn isolate_replaces_existing_symlink() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let base = dir.join("profiles");
        seed_shared(&base);
        let app_system = FakeSystem::default();
        let creds = FakeCreds::with(None, None);
        let app = app_env(&app_system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        // First run creates the links.
        app.dispatch(
            &mut bufs.io(),
            Command::Isolate {
                name: Some("work".to_string()),
                args: vec![],
            },
        )
        .unwrap();
        // Second run sees existing symlinks and replaces them.
        app.dispatch(
            &mut bufs.io(),
            Command::Isolate {
                name: Some("work".to_string()),
                args: vec![],
            },
        )
        .unwrap();
        assert_eq!(app_system.links.borrow().len(), 6);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn isolate_no_name_lists_isolates() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let base = dir.join("profiles");
        seed_shared(&base);
        // A valid isolate (projects is a symlink) and noise entries.
        let work = base.join("work");
        fs::create_dir_all(&work).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(base.join("shared/projects"), work.join("projects")).unwrap();
        fs::create_dir_all(base.join("notlinked")).unwrap();
        fs::write(base.join("loose"), "x").unwrap();
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(
            &mut bufs.io(),
            Command::Isolate {
                name: None,
                args: vec![],
            },
        )
        .unwrap();
        let out = bufs.out();
        assert!(out.contains("isolated profiles in"));
        assert!(out.contains("  work"));
        assert!(!out.contains("notlinked"));
        assert!(out.contains("usage: ccswitch isolate"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn isolate_no_name_none_yet_when_base_absent() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(
            &mut bufs.io(),
            Command::Isolate {
                name: None,
                args: vec![],
            },
        )
        .unwrap();
        assert!(bufs.out().contains("(none yet)"));
        fs::remove_dir_all(&dir).unwrap();
    }

    // ---- seed --------------------------------------------------------------

    #[test]
    fn seed_copies_all_sources() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let src = dir.join("home-claude");
        fs::create_dir_all(src.join("projects").join("p")).unwrap();
        fs::write(src.join("CLAUDE.md"), "notes").unwrap();
        fs::write(src.join("history.jsonl"), "h").unwrap();
        fs::write(src.join("projects").join("p").join("t.jsonl"), "t").unwrap();
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(&mut bufs.io(), Command::Seed { dir: None })
            .unwrap();
        let shared = dir.join("profiles").join("shared");
        assert_eq!(
            fs::read_to_string(shared.join("CLAUDE.md")).unwrap(),
            "notes"
        );
        assert!(shared.join("projects").join("p").join("t.jsonl").exists());
        assert!(bufs.out().contains("done — isolate profiles now share"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn seed_from_explicit_dir_with_nothing_reports_nothing() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let src = dir.join("empty-src");
        fs::create_dir_all(&src).unwrap();
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(
            &mut bufs.io(),
            Command::Seed {
                dir: Some(src.to_string_lossy().into_owned()),
            },
        )
        .unwrap();
        assert!(bufs.out().contains("nothing to seed"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn seed_missing_source_errors() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        // Default source (~/.claude stand-in) does not exist.
        assert!(matches!(
            app.dispatch(&mut bufs.io(), Command::Seed { dir: None }),
            Err(Error::Invalid(m)) if m.contains("not found")
        ));
        fs::remove_dir_all(&dir).unwrap();
    }

    // ---- search ------------------------------------------------------------

    fn search_app<'a>(
        system: &'a FakeSystem,
        creds: &'a FakeCreds,
        store: &'a Store,
        dir: &Path,
    ) -> App<'a> {
        app_env(system, creds, store, dir)
    }

    #[test]
    fn search_requires_csx() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem::default();
        let app = search_app(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        assert!(matches!(
            app.dispatch(&mut bufs.io(), Command::Search { scope: vec![] }),
            Err(Error::Invalid(m)) if m.contains("csx not found")
        ));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn search_requires_fzf() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem {
            has_csx: true,
            ..Default::default()
        };
        let app = search_app(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        assert!(matches!(
            app.dispatch(&mut bufs.io(), Command::Search { scope: vec![] }),
            Err(Error::Invalid(m)) if m.contains("fzf not found")
        ));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn search_default_tool_prepends_scope_and_resumes_claude() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem {
            has_csx: true,
            has_fzf: true,
            current: Some(br#"[{"tool":"claude-code"}]"#.to_vec()),
            sessions: br#"[{"session_id":"s1","tool":"claude-code","msg_count":3}]"#.to_vec(),
            pick: Some("s1\tclaude-code  p  b  (3 msgs)".to_string()),
            ..Default::default()
        };
        let app = search_app(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(&mut bufs.io(), Command::Search { scope: vec![] })
            .unwrap();
        // Scope was prefixed with the discovered tool.
        assert_eq!(
            *system.sessions_scope.borrow(),
            vec!["--tool".to_string(), "claude-code".to_string()]
        );
        let execs = system.execs.borrow();
        assert_eq!(execs[0].0, "claude");
        assert_eq!(execs[0].1, vec!["--resume".to_string(), "s1".to_string()]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn search_explicit_tool_codex_resumes_codex() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem {
            has_csx: true,
            has_fzf: true,
            sessions: br#"[{"session_id":"x9","tool":"codex","msg_count":1}]"#.to_vec(),
            pick: Some("x9\tcodex  p  b  (1 msgs)".to_string()),
            ..Default::default()
        };
        let app = search_app(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(
            &mut bufs.io(),
            Command::Search {
                scope: vec!["--tool".to_string(), "codex".to_string()],
            },
        )
        .unwrap();
        // Explicit scope passed through untouched.
        assert_eq!(
            *system.sessions_scope.borrow(),
            vec!["--tool".to_string(), "codex".to_string()]
        );
        let execs = system.execs.borrow();
        assert_eq!(execs[0].0, "codex");
        assert_eq!(execs[0].1, vec!["resume".to_string(), "x9".to_string()]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn search_unknown_tool_errors() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem {
            has_csx: true,
            has_fzf: true,
            sessions: br#"[{"session_id":"z","msg_count":1}]"#.to_vec(),
            pick: Some("z\t...".to_string()),
            ..Default::default()
        };
        let app = search_app(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        let err = app
            .dispatch(
                &mut bufs.io(),
                Command::Search {
                    scope: vec!["--tool".to_string(), "weird".to_string()],
                },
            )
            .unwrap_err();
        assert!(matches!(err, Error::Invalid(m) if m.contains("don't know how to resume")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn search_no_tool_defaults_to_claude() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(None, None);
        // csx current fails → no default tool; scope passes through empty.
        let system = FakeSystem {
            has_csx: true,
            has_fzf: true,
            current_err: true,
            sessions: br#"[{"session_id":"s","msg_count":1}]"#.to_vec(),
            pick: Some("s\t...".to_string()),
            ..Default::default()
        };
        let app = search_app(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(&mut bufs.io(), Command::Search { scope: vec![] })
            .unwrap();
        assert!(system.sessions_scope.borrow().is_empty());
        assert_eq!(system.execs.borrow()[0].0, "claude");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn search_no_sessions_errors() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem {
            has_csx: true,
            has_fzf: true,
            current_err: true,
            sessions: b"[]".to_vec(),
            ..Default::default()
        };
        let app = search_app(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        assert!(matches!(
            app.dispatch(&mut bufs.io(), Command::Search { scope: vec![] }),
            Err(Error::Invalid(m)) if m.contains("no sessions matched")
        ));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn search_cancelled_pick_is_ok() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem {
            has_csx: true,
            has_fzf: true,
            current_err: true,
            sessions: br#"[{"session_id":"s","msg_count":1}]"#.to_vec(),
            pick: None,
            ..Default::default()
        };
        let app = search_app(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(&mut bufs.io(), Command::Search { scope: vec![] })
            .unwrap();
        assert!(system.execs.borrow().is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn search_empty_id_is_ok() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem {
            has_csx: true,
            has_fzf: true,
            current_err: true,
            sessions: br#"[{"session_id":"s","msg_count":1}]"#.to_vec(),
            pick: Some("\tlabel-only".to_string()),
            ..Default::default()
        };
        let app = search_app(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        app.dispatch(&mut bufs.io(), Command::Search { scope: vec![] })
            .unwrap();
        assert!(system.execs.borrow().is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn search_sessions_spawn_error_propagates() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem {
            has_csx: true,
            has_fzf: true,
            current_err: true,
            sessions_err: true,
            ..Default::default()
        };
        let app = search_app(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        assert!(app
            .dispatch(&mut bufs.io(), Command::Search { scope: vec![] })
            .is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn search_fzf_error_propagates() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem {
            has_csx: true,
            has_fzf: true,
            current_err: true,
            sessions: br#"[{"session_id":"s","msg_count":1}]"#.to_vec(),
            pick_err: true,
            ..Default::default()
        };
        let app = search_app(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        assert!(app
            .dispatch(&mut bufs.io(), Command::Search { scope: vec![] })
            .is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn search_exec_failure_propagates() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem {
            has_csx: true,
            has_fzf: true,
            current_err: true,
            sessions: br#"[{"session_id":"s","msg_count":1}]"#.to_vec(),
            pick: Some("s\tlabel".to_string()),
            exec_err: true,
            ..Default::default()
        };
        let app = search_app(&system, &creds, &store, &dir);
        let mut bufs = Bufs::new();
        assert!(app
            .dispatch(&mut bufs.io(), Command::Search { scope: vec![] })
            .is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    // ---- output-write failures --------------------------------------------
    //
    // Every message goes out through a fallible writer; these drive the error
    // arm of each `writeln!` by failing on a marker substring.

    /// A writer that errors on any chunk containing `marker`.
    struct FailOn {
        marker: &'static str,
    }

    impl Write for FailOn {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if std::str::from_utf8(buf).is_ok_and(|s| s.contains(self.marker)) {
                return Err(std::io::Error::other("write failed"));
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Run `command` with `out`/`err` writers that fail on the given markers,
    /// asserting the write error surfaces.
    fn assert_write_error(
        app: &App<'_>,
        out_marker: &'static str,
        err_marker: &'static str,
        command: Command,
    ) {
        let mut out = FailOn { marker: out_marker };
        let mut err = FailOn { marker: err_marker };
        let mut io = Io {
            out: &mut out,
            err: &mut err,
        };
        assert!(app.dispatch(&mut io, command).is_err());
    }

    #[test]
    fn fail_on_writer_flush_is_a_noop() {
        let mut w = FailOn { marker: "x" };
        assert!(w.flush().is_ok());
    }

    #[test]
    fn write_errors_surface_from_out_messages() {
        let dir = temp_dir();
        write_config(&dir, &config_json("A", "OA", "OrgMarker@x", "OrgMarker"));
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(Some("BLOB"), Some("attr"));
        let system = FakeSystem {
            login_ok: true,
            ..Default::default()
        };
        let base = dir.join("profiles");
        seed_shared(&base);
        let seed_src = dir.join("home-claude");
        fs::create_dir_all(&seed_src).unwrap();
        fs::write(seed_src.join("CLAUDE.md"), "notes").unwrap();
        let app = app_env(&system, &creds, &store, &dir);

        // add — the "signing in" banner.
        assert_write_error(
            &app,
            "signing in as",
            "\0",
            Command::Add {
                name: "dev".to_string(),
                force: false,
            },
        );
        // current — email/org line.
        assert_write_error(&app, "OrgMarker", "\0", Command::Current);
        // list (empty) — the hint line.
        assert_write_error(&app, "no profiles yet", "\0", Command::List);
        // isolate — the "launching" banner (shared is seeded, so no prompt).
        assert_write_error(
            &app,
            "launching isolated",
            "\0",
            Command::Isolate {
                name: Some("work".to_string()),
                args: vec![],
            },
        );
        // seed — the initial "seeding shared memory" line.
        assert_write_error(
            &app,
            "seeding shared memory",
            "\0",
            Command::Seed { dir: None },
        );
        // seed — the closing "done" line (earlier lines pass through).
        assert_write_error(&app, "done — isolate", "\0", Command::Seed { dir: None });

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_errors_surface_from_list_org_row() {
        let dir = temp_dir();
        write_config(&dir, &config_json("A", "OA", "a@x", "Org A"));
        let store = Store::new(dir.join("accounts"));
        seed_profile(&store, "dev", "A", "OA", "a@x");
        let creds = FakeCreds::with(Some("LIVE"), None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        // The profile row carries org "Org", so failing on it hits the org branch.
        assert_write_error(&app, "Org", "\0", Command::List);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_errors_surface_from_err_messages() {
        let dir = temp_dir();
        write_config(&dir, &config_json("B", "OB", "b@x", "Org B"));
        let store = Store::new(dir.join("accounts"));
        seed_profile(&store, "dev", "A", "OA", "a@x");
        let creds = FakeCreds::with(Some("LIVE"), Some("attr"));
        let system = FakeSystem {
            running: true,
            ..Default::default()
        };
        let app = app_env(&system, &creds, &store, &dir);
        // use — the "a running claude" warning goes to err.
        assert_write_error(
            &app,
            "\0",
            "warning: a running",
            Command::Use {
                name: "dev".to_string(),
            },
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_errors_surface_from_isolate_warnings() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem {
            confirm_yes: true,
            ..Default::default()
        };
        let app = app_env(&system, &creds, &store, &dir);
        // First warning line to err (memory empty).
        assert_write_error(
            &app,
            "\0",
            "shared isolate memory is empty",
            Command::Isolate {
                name: Some("work".to_string()),
                args: vec![],
            },
        );
        // Second warning line to err (first passes through).
        assert_write_error(
            &app,
            "\0",
            "history or CLAUDE.md",
            Command::Isolate {
                name: Some("work".to_string()),
                args: vec![],
            },
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_errors_surface_from_skip_symlink_warning() {
        let dir = temp_dir();
        let store = Store::new(dir.join("accounts"));
        let base = dir.join("profiles");
        seed_shared(&base);
        // Pre-create a non-symlink projects dir so link_into warns to err.
        fs::create_dir_all(base.join("work").join("projects")).unwrap();
        let creds = FakeCreds::with(None, None);
        let system = FakeSystem::default();
        let app = app_env(&system, &creds, &store, &dir);
        assert_write_error(
            &app,
            "\0",
            "is not a symlink",
            Command::Isolate {
                name: Some("work".to_string()),
                args: vec![],
            },
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
