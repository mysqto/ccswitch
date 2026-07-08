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
| `model.rs`       | Domain types: `Account` (uuid/org/email/org-name) and `Profile` (persisted `account.json`).      | — |
| `error.rs`       | `Error` / `Result` (`ProfileNotFound`, `ReservedName`, `Invalid`, wrapped `Io`/`Json`).          | — |
| `config.rs`      | Read/splice `~/.claude.json`: extract + reinsert the `{oauthAccount, userID}` identity, leaving every other key untouched. Plain-file JSON, temp-tested. | — |
| `creds.rs`       | `CredentialStore` trait + the `FileStore` (`<config>/.credentials.json`) fallback used off macOS. Temp-tested. | `CredentialStore` |
| `creds_shim.rs`  | Real macOS Keychain adapter (`security` binary) + `platform_store` `cfg` selection.              | `CredentialStore` → **shim** |
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
