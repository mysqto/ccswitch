# Changelog

All notable changes to ccswitch are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- **Switching now works on headless and pure-SSH machines.** Claude Code's macOS
  credential store is a composite — Keychain first, falling back to plaintext
  `~/.claude/.credentials.json`, with a failed Keychain write migrating the
  credential into that file. Over SSH the login Keychain cannot be unlocked
  without a GUI prompt (`security` exits 36, `errSecInteractionNotAllowed`), so
  Claude Code lives entirely in the file. `ccswitch` only ever looked at the
  Keychain, so `save` reported "no active credential" on a machine that was
  signed in, `sync_current` silently dropped rotated tokens, and a switch left
  `~/.claude.json` naming the new account while the live token still belonged to
  the old one — which Claude Code reports as **not logged in**. The macOS store
  now mirrors Claude Code's arbitration and keeps exactly one authoritative
  copy: a successful Keychain write deletes the plaintext file, a failed one
  deletes the Keychain item, so a stale copy can never shadow the switch from
  the other kind of session.
- **The Keychain `acct` attribute is captured again.** `security
  find-generic-password -g` prints attributes on stdout and the password on
  stderr; `account_attr` parsed stderr, so it always came back empty and every
  profile stored `keychain_account: ""`. (Writes fell through to `$USER`, which
  is what Claude Code looks items up by, so this was invisible.)

- **`$CLAUDE_CONFIG_DIR` is honored.** It moves `.claude.json` and the
  credential fallback, and — the part that is easy to miss — it *renames the
  Keychain item*: Claude Code namespaces the service with the first eight hex
  characters of `sha256($CLAUDE_CONFIG_DIR)`. `ccswitch` hardcoded
  `"Claude Code-credentials"`, so with the variable set it read and wrote a
  **different account's** credential while patching the wrong config file. This
  affected ordinary use, because `ccswitch isolate` launches `claude` with
  `CLAUDE_CONFIG_DIR` set to the isolate directory. The related
  `$CLAUDE_SECURESTORAGE_CONFIG_DIR` and `$CLAUDE_CODE_CUSTOM_OAUTH_URL` feed
  the same derivation and are honored too.
- **The OAuth token no longer appears in `security`'s argv**, where `ps` exposed
  it to every user on the machine for the duration of a switch. It is now
  hex-encoded and piped to `security -i`, with the same argv fallback Claude
  Code uses for a blob past the stdin limit. Account attributes are also
  sanitized the way Claude Code sanitizes them, so the two cannot end up
  addressing different items.

### Changed

- Profile and isolate storage now default to `accounts/` and `profiles/` inside
  Claude Code's **resolved** config directory rather than always `~/.claude`.
  With `$CLAUDE_CONFIG_DIR` unset this is the same path as before. With it set,
  each config directory gets its own profiles; set `$CCSWITCH_HOME` to a fixed
  path to share one set across all of them.

### Added

- `$CCSWITCH_CREDENTIALS=file` (or `plaintext`) forces the plaintext backend and
  skips the Keychain entirely — for a Mac you only ever reach over SSH.
- `ccswitch save` now says when the Keychain is locked or unreachable, instead
  of reporting a reachable-but-empty store and a locked one identically.

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
