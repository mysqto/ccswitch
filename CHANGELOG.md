# Changelog

All notable changes to ccswitch are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

_Nothing yet._

## [0.1.0] — 2026-07-08

First release of the Rust CLI, superseding the original fish plugin (now under
`legacy-fish/`).

### Added

- **Account switching** — `save`, `use`, and the bare `ccswitch <name>
  [args...]` form snapshot both halves of a Claude Code identity (the OAuth
  credential and the `~/.claude.json` `oauthAccount` + `userID`) into a plain
  profile directory and restore them as a unit, launching `claude` on the bare
  form.
- **`add`** — sign in to a new account (`claude auth login`) and save it in one
  step; `--force` on `add`/`save` overwrites an existing profile.
- **`list`, `current`, `rm`** — enumerate profiles (`*` marks the active one),
  show the active account, and delete a profile; with aliases `ls`, `whoami`,
  and `remove`/`delete`.
- **Token-rotation fix** — on every switch the outgoing credential is
  re-snapshotted into **every** profile that shares its per-account refresh
  token (`TokenScope::PerAccount`), so an actively used account never strands a
  sibling profile with a rotated-out token.
- **Concurrent sessions** — `isolate` runs a session under a per-profile
  `CLAUDE_CONFIG_DIR` with `projects/`, `history.jsonl`, and `CLAUDE.md`
  symlinked to a shared directory, and `seed` imports that shared memory from
  `~/.claude` (or a given dir). Empty shared memory triggers a confirm.
- **`search`** — bridges to [`csx`](https://github.com/mysqto/csx) + `fzf` to
  fuzzy-pick and resume a past session, defaulting the scope to the active tool
  and resuming with the right command per tool (`claude`, `codex`).
- **Cross-shell completions** — `completions <shell>` for bash, zsh, fish,
  PowerShell, and elvish.
- **Cross-platform credentials** — macOS Keychain via the `security` binary,
  with a `~/.claude/.credentials.json` file store fallback elsewhere.
- **Docs & distribution** — README, `AGENTS.md`, a GitHub release workflow with
  a cross-compile matrix, and an auto-bumped Homebrew cask.

### Notes

- Test suite covers decision logic to ≥98% line and region; OS/network I/O
  lives in `*_shim.rs` files behind traits (see `AGENTS.md`).

[Unreleased]: https://github.com/mysqto/ccswitch/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/mysqto/ccswitch/releases/tag/v0.1.0
