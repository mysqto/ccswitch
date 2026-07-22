# Changelog

All notable changes to ccswitch are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

_Nothing yet._

## [0.1.3] — 2026-07-16

### Fixed

- **Same-login, multi-org switching is no longer cosmetic.** The default token
  scope is now `PerAccountOrg` (one credential per `(account, org)`), not
  `PerAccount`. A Claude Code OAuth token is bound **server-side to the org it
  was minted under** — it is opaque and the server re-derives the org from it at
  session start, overwriting `~/.claude.json`. The old `PerAccount` scope shared
  one token across a login's orgs, so `sync_current` propagated one org's token
  into the sibling profile; every "switch" then only relabelled `~/.claude.json`
  while real sessions ran under the token's minting org. Each profile now keeps
  its own org-scoped token, and `sync_current` re-snapshots **only the outgoing
  profile**. (Existing profiles saved before this release share one token and
  must be re-provisioned: `claude /login` into the right org, then
  `ccswitch save <name> --force`.)

### Removed

- **The daemon-stop on switch (v0.1.1/v0.1.2).** It addressed a misdiagnosis:
  there is no persistent Claude Code daemon holding auth, and stopping it never
  affected switching. v0.1.2's worker-kill also needlessly ended background
  sessions. Removed entirely.

## [0.1.2] — 2026-07-08

### Changed

- **Daemon stop now includes session workers.** v0.1.1 stopped only the daemon
  supervisor (`--keep-workers`), but a kept worker holds in memory the account
  it was started under — so `claude --resume` reattached to the old worker and
  the switch appeared to have no effect (the resumed session kept the previous
  org). The switch now runs `claude daemon stop --any` (no `--keep-workers`), so
  every subsequent session, resumed or fresh, picks up the switched account.
  Trade-off: switching ends any detached background Claude Code sessions.

## [0.1.1] — 2026-07-08

### Fixed

- **Daemon-aware switching** — recent Claude Code releases keep a background
  daemon running that caches each account's auth in memory, so a profile switch
  did not take effect until the daemon exited. Every switch (`use` and the bare
  `ccswitch <name>` form) now stops the daemon's supervisor first via `claude
  daemon stop --any --keep-workers`, so the next session re-reads the restored
  credentials while any detached background sessions keep running. It is
  best-effort: an older Claude Code, a missing binary, or no running daemon
  never blocks the switch (a warning is printed and the switch proceeds).

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
