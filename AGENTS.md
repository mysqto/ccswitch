# AGENTS.md — working in the `ccswitch` repo

A guide for coding agents. Read it before touching this tree. `ccswitch` is a
cross-shell CLI that switches between multiple Claude Code accounts by
snapshotting and restoring each account's OAuth credential together with its
`~/.claude.json` identity. The whole design exists to keep one invariant true:

> **Every real OS/network side effect sits behind a trait whose only real
> implementation lives in a `*_shim.rs` file. All decision logic lives outside
> shims and is unit-tested. Coverage target: ≥98% line AND region.**

If you internalize one thing, make it that.

---

## Build / test / lint

```sh
cargo build
cargo test
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
```

All four must be green before you finish. `cargo fmt` and a clippy run with
`-D warnings` are non-negotiable.

Dependencies are lean and pinned in `Cargo.toml`. The sanctioned set: `clap`
(derive) + `clap_complete` for the CLI and completions, `serde`/`serde_json`
for the JSON config and profile files, and `thiserror`/`anyhow` for errors. Do
not add crates casually — there is no database, HTTP, or async runtime here.

---

## Coverage discipline (the important part)

Run coverage with Homebrew LLVM so `llvm-cov`/`llvm-profdata` match the
toolchain:

```sh
LLVM_COV="$(brew --prefix llvm)/bin/llvm-cov" \
LLVM_PROFDATA="$(brew --prefix llvm)/bin/llvm-profdata" \
cargo llvm-cov \
  --all-features \
  --ignore-filename-regex '(_shim\.rs$|/main\.rs$)' \
  --summary-only
```

**Ignore regex:** `(_shim\.rs$|/main\.rs$)`. That is the coverage boundary — the
`*_shim.rs` adapters plus the trivial `main.rs` entry point (which only
delegates to `ccswitch::cli_shim::run`). Everything else must be covered to
≥98% line **and** region.

Rules that keep coverage reachable:

- A `*_shim.rs` file holds a trait's **real** adapter and *nothing a test needs
  to reach*: no branching, no parsing, no decisions. If you find yourself
  writing an `if`/`match` that matters inside a shim, it belongs in a non-shim
  module behind the trait.
- Reading and writing plain files under a **temp root** is testable — do it
  directly, no shim. Only spawning processes (`claude`, `csx`, `fzf`),
  creating symlinks, terminal prompting, the macOS Keychain (`security`
  binary), and replacing the process via `exec` need shims.
- Every non-shim module carries its own `#[cfg(test)] mod tests` exercising all
  branches with fakes and `std::env::temp_dir()`-based temp directories.

---

## Architecture / module map

Ports (traits) are consumed by pure logic; the matching `*_shim.rs` supplies the
one real adapter. Data flow: **`cli::parse_from` → `Command` → `App::dispatch`
→ (`Switcher` over `CredentialStore` + `config` + `Store`) and/or the `System`
port → output.**

| Module           | Role                                                                                             | Port(s) → shim |
| ---------------- | ------------------------------------------------------------------------------------------------ | -------------- |
| `sha256.rs`      | A dependency-free SHA-256 (`hex_digest`). Exists only because Claude Code namespaces its Keychain service with `sha256($CLAUDE_CONFIG_DIR)[0..8]`, and the dependency budget does not stretch to `sha2` + six transitive crates for one 8-char string. Checked against the FIPS vectors. | — |
| `claude_env.rs`  | `ClaudeEnv` — resolves everything Claude Code's own env vars move: the config dir, `.claude.json`, the credential-fallback dir, and the **Keychain service name**. See *Config directories* below. | — |
| `model.rs`       | Domain types: `Account` (uuid/org/email/org-name) and `Profile` (persisted `account.json`).      | — |
| `error.rs`       | `Error` / `Result` (`ProfileNotFound`, `ReservedName`, `Invalid`, wrapped `Io`/`Json`).          | — |
| `config.rs`      | Read/splice `~/.claude.json`: extract + reinsert the `{oauthAccount, userID}` identity, leaving every other key untouched. Plain-file JSON, temp-tested. | — |
| `creds.rs`       | `CredentialStore` + `Keychain` traits, the `FileStore` (`<config>/.credentials.json`), and `KeychainStore` — the composite that mirrors Claude Code's own Keychain-over-plaintext arbitration (see *Headless machines* below). Also `security` exit-code classification, `acct` parsing, and `Backend`/`select_store`. Temp-tested against a fake `Keychain`. | `CredentialStore`, `Keychain` |
| `creds_shim.rs`  | `SecurityBinary` — a single `run(args, stdin)` that spawns `security` and returns unclassified exit status + streams — plus `platform_store` (`cfg` + env, no decisions). | `Keychain` → **shim** |
| `store.rs`       | The on-disk profile store under `$CCSWITCH_HOME`; `TokenScope` (`PerAccount` / `PerAccountOrg`) — the knob behind the auth-loss fix. Temp-tested. | — |
| `switch.rs`      | `Switcher` — save/activate orchestration over `creds` + `config` + `store`; re-snapshots the outgoing credential into every token-sharing sibling. | consumes `CredentialStore` |
| `cli.rs`         | clap types, the `Command` model, `App::dispatch` + every handler, `System` port, path resolution, symlink planning, `search`/`isolate`/`seed` logic, completion generation. | `System` |
| `cli_shim.rs`    | `RealSystem` (spawn `claude`/`csx`/`fzf`, symlinks, prompts, `exec`) + `run` — the one impure entry point (real env, `$HOME`/`$CCSWITCH_*`, stdio, platform stores). | `System` → **shim** |
| `main.rs`        | Binary entry point; delegates to `cli_shim::run`.                                                | — (ignored) |

The `System` trait in `cli.rs` is the seam that keeps the whole command surface
testable: handlers call `claude_login`, `command_exists`, `claude_is_running`,
`make_symlink`, `confirm`, `csx_current`, `csx_sessions`, `fzf_pick`, and
`exec` through it, and the tests drive a `FakeSystem` recording every call.

---

## The auth-loss fix (why `TokenScope` exists)

The Claude Code OAuth **refresh token rotates per account**, shared across
every organization a single login can operate in (org selection is client-side
state in `~/.claude.json`, not part of the credential). The original tool
re-snapshotted the live credential into only the one profile whose
`(accountUuid, organizationUuid)` matched — so two profiles for the same login
but different orgs each held a copy of the one shared token, and the first
refresh under either org stranded the sibling with a dead one.

`Switcher` closes that gap: before switching away it re-snapshots the live
credential into **every** profile that shares the outgoing account's token, as
selected by `TokenScope`. Production wires `TokenScope::PerAccount` (group by
`accountUuid` alone), so a rotation under any org keeps every sibling current.
Preserve this behavior; it is the reason the Rust tool exists.

---

## Headless machines (why the credential store is a composite)

Claude Code's macOS credential store is `keychain-with-plaintext-fallback`: it
reads the Keychain first and falls back to `~/.claude/.credentials.json`, and a
failed Keychain write migrates the credential into that file. Over SSH the
login Keychain cannot be unlocked without a GUI prompt (`security` exits 36,
`errSecInteractionNotAllowed`), so Claude Code is living entirely in the file.

A Keychain-only `ccswitch` is blind to that: `read` sees nothing (so `save`
claims you are signed out and `sync_current` silently drops a rotated token)
and `write` cannot land the incoming credential, leaving `~/.claude.json`
naming the new account while the live token is still the old one — which Claude
Code reports as not logged in.

`KeychainStore` therefore mirrors the same arbitration, and in particular keeps
**one** authoritative copy: a successful Keychain write deletes the plaintext
file, and a failed one deletes the Keychain item. Dropping either half of that
lets a stale copy shadow the switch from the other kind of session. Preserve
this; `$CCSWITCH_CREDENTIALS=file` forces the plaintext path outright.

Note that `security find-generic-password -g` prints attributes on **stdout**
and the password on stderr — parsing the wrong stream silently yields no `acct`
attribute forever.

The credential must never reach `security`'s argv, which `ps` exposes to every
user on the machine. `KeychainStore::store_password` hex-encodes it and pipes
`add-generic-password … -X <hex>` to `security -i`, dropping to argv only past
the 4032-character stdin limit — the same boundary Claude Code uses. The
`Keychain` port is deliberately a single `run(args, stdin)` so that this choice
is a decision in `creds.rs` with a test asserting the secret never appears in
argv, not something buried in the shim.

---

## Config directories (why `claude_env.rs` exists)

`$CLAUDE_CONFIG_DIR` moves `.claude.json` and the credential fallback, and it
**renames the Keychain item**: Claude Code appends
`-${sha256(dir).hex[0..8]}` to the service name. A tool that hardcodes
`"Claude Code-credentials"` therefore reads and writes a *different account's*
credential whenever the variable is set — including inside `ccswitch isolate`,
which launches `claude` with it set to the isolate directory.

`ClaudeEnv` owns that computation; `cli_shim::run` reads the variables and
everything else takes resolved paths and a service `String`. Two relatives feed
the same derivation: `$CLAUDE_SECURESTORAGE_CONFIG_DIR` (relocates only the
credential store, and "defined but empty" means the *default* store, not a new
namespace) and `$CLAUDE_CODE_CUSTOM_OAUTH_URL` (a `-custom-oauth` infix on both
the config file name and the service).

Profile and isolate roots hang off the resolved config dir too, so
`cli::ccswitch_home`/`isolate_home` take that directory, not `$HOME`.

Claude Code NFC-normalizes the directory before hashing; we hash the bytes as
given, which agrees for any already-NFC path. Documented, not fixed — NFC would
mean a Unicode dependency.

---

## Adding a feature

1. **Decide where the logic goes.** Any real side effect (a new process to
   spawn, a new prompt, a new file to watch) becomes a method on the `System`
   trait in `cli.rs`, implemented once in `RealSystem` (`cli_shim.rs`). Pure
   decision logic — parsing, validation, path math, JSON shaping — goes in the
   plain module and is unit-tested with a fake. Never put a decision in a
   `_shim.rs`.
2. **A new subcommand:** add a variant to `Sub` (clap) and `Command` in
   `cli.rs`, map it in `From<Option<Sub>>`, add a handler on `App`, route it in
   `dispatch`, and extend `help_text()` and `RESERVED` if it introduces a new
   word. Cover the parse, the dispatch, and each error branch.
3. **A new credential backend:** implement `CredentialStore` in `creds.rs` (if
   temp-testable) or add the real adapter to `creds_shim.rs` and select it in
   `platform_store`.
4. **Run the gate:** `cargo build && cargo test`, then `cargo fmt`,
   `cargo clippy --all-targets --all-features -- -D warnings`, then the
   coverage command above — line and region must stay ≥98%.

---

## Distribution

`.github/workflows/release.yml` cuts a release on a `v*` tag (or manual
`workflow_dispatch` with a validated `tag` input): it cross-compiles the four
targets (`aarch64`/`x86_64` × macOS/Linux — the Intel mac is built on the
Apple-Silicon `macos-14` runner), publishes per-arch `.tar.gz` + `.sha256`
assets to a GitHub release, and regenerates `Casks/ccswitch.rb` with the real
checksums (a `github_latest` livecheck keeps it tracking the latest tag). The
binary name is `ccswitch`.
