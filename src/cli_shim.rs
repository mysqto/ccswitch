//! Real process-spawning adapter and program entry point.
//!
//! Side-effecting implementation of the [`crate::cli::System`] port: launching
//! `claude`, `csx`, and `fzf`, creating symlinks, prompting on the terminal,
//! and replacing the current process via `exec`. This file also owns the
//! real-environment wiring in [`run`] (argv parsing, `$HOME`/`$CCSWITCH_*`
//! resolution, and building the platform stores). It contains no decision
//! logic — every branch of behavior lives in [`crate::cli`] — and is excluded
//! from coverage.

use crate::cli::{self, App, Io, Paths, System};
use crate::creds_shim::platform_store;
use crate::error::{Error, Result};
use crate::store::{Store, TokenScope};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as Proc, Stdio};

/// The real system adapter.
struct RealSystem;

impl System for RealSystem {
    fn claude_login(&self) -> Result<bool> {
        Ok(Proc::new("claude")
            .args(["auth", "login"])
            .status()?
            .success())
    }

    fn command_exists(&self, program: &str) -> bool {
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join(program).is_file())
        })
    }

    fn claude_is_running(&self) -> bool {
        Proc::new("pgrep")
            .args(["-x", "claude"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn make_symlink(&self, target: &Path, link: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(link);
            std::os::unix::fs::symlink(target, link)?;
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = (target, link);
            Err(Error::Invalid(
                "symlinks unsupported on this platform".to_string(),
            ))
        }
    }

    fn confirm(&self, prompt: &str) -> Result<bool> {
        let mut stderr = std::io::stderr();
        write!(stderr, "{prompt}")?;
        stderr.flush()?;
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        let answer = line.trim().to_ascii_lowercase();
        Ok(answer == "y" || answer == "yes")
    }

    fn csx_current(&self) -> Result<Vec<u8>> {
        Ok(Proc::new("csx")
            .args(["current", "--json"])
            .output()?
            .stdout)
    }

    fn csx_sessions(&self, scope: &[String]) -> Result<Vec<u8>> {
        Ok(Proc::new("csx")
            .args(["sessions", "--json"])
            .args(scope)
            .output()?
            .stdout)
    }

    fn fzf_pick(&self, rows: &str, preview: &str) -> Result<Option<String>> {
        let mut child = Proc::new("fzf")
            .args([
                "--with-nth=2..",
                "--delimiter=\t",
                "--preview",
                preview,
                "--preview-window=right,60%,wrap",
                "--prompt=session> ",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .ok_or_else(|| Error::Invalid("fzf stdin unavailable".to_string()))?
            .write_all(rows.as_bytes())?;
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Ok(None);
        }
        let picked = String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string();
        if picked.is_empty() {
            Ok(None)
        } else {
            Ok(Some(picked))
        }
    }

    fn exec(&self, program: &str, args: &[String], envs: &[(String, String)]) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let mut cmd = Proc::new(program);
            cmd.args(args);
            for (key, value) in envs {
                cmd.env(key, value);
            }
            Err(cmd.exec().into())
        }
        #[cfg(not(unix))]
        {
            let status = Proc::new(program)
                .args(args)
                .envs(envs.iter().cloned())
                .status()?;
            if status.success() {
                Ok(())
            } else {
                Err(Error::Invalid(format!("{program} exited with failure")))
            }
        }
    }
}

/// The home directory, falling back to the current directory.
fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// Program entry point: parse arguments, resolve the environment, and dispatch.
///
/// # Errors
///
/// Returns an error if the selected subcommand fails.
pub fn run() -> anyhow::Result<()> {
    let command = match cli::parse_from(std::env::args_os()) {
        Ok(command) => command,
        Err(err) => err.exit(),
    };

    let home = home_dir();
    let claude = cli::claude_dir(&home);
    let store = Store::new(cli::ccswitch_home(
        std::env::var("CCSWITCH_HOME").ok(),
        &home,
    ));
    let creds = platform_store(claude.clone());
    let paths = Paths {
        config: cli::config_path(&home),
        isolate_base: cli::isolate_home(std::env::var("CCSWITCH_ISOLATE_HOME").ok(), &home),
        seed_default: claude,
    };
    let system = RealSystem;
    let app = App::new(
        &system,
        creds.as_ref(),
        &store,
        paths,
        TokenScope::PerAccount,
    );

    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let mut out = stdout.lock();
    let mut err = stderr.lock();
    let mut io = Io {
        out: &mut out,
        err: &mut err,
    };
    app.dispatch(&mut io, command)?;
    Ok(())
}
