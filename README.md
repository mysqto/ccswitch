# ccswitch

**Switch between multiple Claude Code accounts — personal, work, different
orgs — and jump straight into a session.**

`ccswitch` is a small, fast, cross-shell command-line tool. An account is two
things that must travel together: the OAuth credential (in the macOS Keychain,
or `~/.claude/.credentials.json` elsewhere) and the identity Claude Code
validates it against (`oauthAccount` + `userID` inside `~/.claude.json`).
`ccswitch` snapshots both into a plain profile directory and restores them as a
unit, so one command becomes an account and drops you into `claude`.

```sh
ccswitch work          # switch to the "work" account and start claude
ccswitch personal -c   # switch to "personal", launch claude with -c
ccswitch use work      # just switch, don't launch
ccswitch list          # every saved profile (* = active)
```

- **One binary, every shell.** A single Rust binary with completions for bash,
  zsh, fish, and PowerShell.
- **Token-rotation safe.** OAuth refresh tokens rotate per *login*, shared
  across every org that login can see; `ccswitch` re-snapshots the outgoing
  credential into every sibling profile on each switch, so an actively used
  account never goes stale.
- **Concurrent sessions.** Run two accounts at once in separate terminals with
  isolated config dirs but shared project memory and history.
- **Session recall.** `ccswitch search` bridges to
  [`csx`](https://github.com/mysqto/csx) + `fzf` to fuzzy-pick and resume any
  past session.

> This Rust CLI supersedes the original fish plugin, now preserved under
> [`legacy-fish/`](legacy-fish/). The commands are the same; the tool is now a
> single portable binary rather than a fish-only function.

---

## Install

### Homebrew (macOS & Linux)

```sh
brew tap mysqto/ccswitch https://github.com/mysqto/ccswitch
brew trust mysqto/ccswitch     # newer Homebrew requires trusting a third-party tap
brew install ccswitch          # formula (recommended) — also installs shell completions
# or:
brew install --cask ccswitch   # cask — installs the binary only (no completions)
```

> Recent Homebrew refuses to load a cask/formula from an untrusted tap until you
> run `brew trust <tap>` (a one-time consent per tap). If you skip it you'll see
> "Refusing to load … from untrusted tap"; `brew trust mysqto/ccswitch` fixes it.

The tap ships **both** a formula and a cask, auto-regenerated with real
checksums on each release. Prefer `brew install ccswitch` (the **formula**): it
runs `ccswitch completions {bash,zsh,fish}` during install and drops each script
into Homebrew's completion directories, so tab-completion works right away. The
`--cask` route installs just the binary — use it if you don't want the formula,
and set up completions yourself (see [Shell completions](#shell-completions)). Install one,
not both — they each provide a `ccswitch` binary.

### From source with Cargo

```sh
cargo install --git https://github.com/mysqto/ccswitch   # or, in a clone: cargo install --path .
```

### Prebuilt binary

Download the tarball for your platform from the [latest release][releases],
extract it, and put `ccswitch` on your `PATH`. Assets are named
`ccswitch-<tag>-<target>.tar.gz`, each with a matching `.sha256`:

| Platform | Target |
| --- | --- |
| macOS Apple Silicon | `aarch64-apple-darwin` |
| macOS Intel | `x86_64-apple-darwin` |
| Linux x86_64 | `x86_64-unknown-linux-gnu` |
| Linux ARM64 | `aarch64-unknown-linux-gnu` |

Every method produces a single `ccswitch` binary with no runtime dependencies
(the optional `search` bridge aside).

[releases]: https://github.com/mysqto/ccswitch/releases/latest

---

## Commands

Run `ccswitch` (or `ccswitch help`) for usage. Every command is below.

| Command | Action |
| --- | --- |
| `ccswitch <name> [args...]` | Switch to `<name>`, then start `claude` (extra args passed through) |
| `ccswitch use <name>` | Switch without launching |
| `ccswitch add <name> [--force]` | Sign in to a new account (`claude auth login`) and save it as `<name>` |
| `ccswitch save <name> [--force]` | Snapshot the current account as `<name>` |
| `ccswitch isolate <name> [args...]` | Run a **concurrent** session isolated to `<name>` (own credential, shared memory) |
| `ccswitch isolate` | With no name, list existing isolated profiles |
| `ccswitch seed [dir]` | Seed the shared isolate memory from `~/.claude` (or `dir`) |
| `ccswitch search [scope...]` | Fuzzy-pick a past session (via `csx` + `fzf`) and resume it |
| `ccswitch list` | List saved profiles (`*` marks the active one) |
| `ccswitch current` | Show the active account |
| `ccswitch rm <name>` | Delete a saved profile |
| `ccswitch completions <shell>` | Print a completion script to stdout |
| `ccswitch help` | Show usage |

**Aliases:** `iso` = `isolate`, `s` = `search`, `ls` = `list`, `whoami` =
`current`, `remove`/`delete` = `rm`.

**`--force`** (on `add` and `save`) overwrites an existing profile of the same
name; without it, saving over an existing profile is refused.

Profile names may not collide with a reserved subcommand word (`save`, `add`,
`isolate`, `iso`, `seed`, `search`, `s`, `list`, `ls`, `current`, `whoami`,
`use`, `rm`, `remove`, `delete`, `help`).

### Environment

| Variable | Purpose | Default |
| --- | --- | --- |
| `CCSWITCH_HOME` | Where saved profiles live. | `~/.claude/accounts` |
| `CCSWITCH_ISOLATE_HOME` | Where isolated profiles + shared memory live. | `~/.claude/profiles` |

---

## Shell completions

`ccswitch completions <shell>` prints a completion script to stdout for
`bash`, `zsh`, `fish`, `powershell` (and `elvish`). Install it once for your
shell:

**bash** — add to `~/.bashrc`:

```sh
source <(ccswitch completions bash)
```

**zsh** — write into a directory on your `$fpath` (before `compinit`):

```sh
ccswitch completions zsh > "${fpath[1]}/_ccswitch"
```

**fish**:

```fish
ccswitch completions fish > ~/.config/fish/completions/ccswitch.fish
```

**PowerShell** — add to your `$PROFILE`:

```powershell
ccswitch completions powershell | Out-String | Invoke-Expression
```

---

## Walkthrough

A complete run, from a fresh install to switching daily.

### 1. Save the account you're already signed into

```sh
claude auth status        # confirm who you're logged in as
ccswitch save personal    # snapshot the active account as "personal"
ccswitch list
# * personal          you@gmail.com
```

The `*` marks the currently active account.

### 2. Add a second account

`ccswitch add` signs you in to another account and saves it in one step:

```sh
ccswitch add work         # runs `claude auth login`, then saves it as "work"
```

Both are now saved:

```sh
ccswitch list
#   personal          you@gmail.com
# * work              you@company.com (Acme)
```

### 3. Switch and work — the daily loop

```sh
ccswitch work             # become "work" and start a claude session
ccswitch personal -c      # become "personal", continue the last conversation
ccswitch use work         # just switch, don't launch anything
ccswitch current          # you@company.com (Acme)
```

One command becomes an account and drops you into a session. Switching also
re-snapshots the account you're leaving, so rotating tokens never go stale.

### Same login, multiple orgs

If one login (e.g. a Claude Team account) belongs to several organizations,
each org is a separate profile — Claude Code issues a distinct org-scoped token
per org. Save one after switching org inside Claude Code (`/login`):

```sh
ccswitch save org-a      # while the first org is active
# switch org in Claude Code, then:
ccswitch save org-b
```

`ccswitch list` distinguishes them by account **and** organization, so the same
email can appear more than once with only the active org starred.

---

## Concurrent sessions (isolate + seed)

`ccswitch <name>` swaps a single machine-global account, so it is **sequential**
— one active account at a time. To run **two accounts at once** (two terminals),
use `ccswitch isolate`, which gives each profile its own
[`CLAUDE_CONFIG_DIR`](https://code.claude.com/docs/en/settings):

```sh
ccswitch seed             # once: copy your ~/.claude memory + history into shared/
ccswitch isolate work     # terminal 1 — sign in once as work
ccswitch isolate personal # terminal 2 — sign in once as personal, at the same time
```

Each profile keeps its **own credential and identity**, but memory and history
are **shared** — `isolate` symlinks `projects/` (session transcripts + the
`memory/` files), `history.jsonl`, and `CLAUDE.md` from each profile to a common
`shared/` directory. So both accounts see the same project memory and past
sessions while staying logged in as different users.

Layout (centralized under `~/.claude/profiles`, override with
`$CCSWITCH_ISOLATE_HOME`):

```
~/.claude/profiles/
  ├── shared/            # one real copy of the shared memory/history
  │   ├── projects/
  │   ├── history.jsonl
  │   └── CLAUDE.md
  ├── work/              # CLAUDE_CONFIG_DIR for "work"
  │   ├── projects      -> ../shared/projects
  │   ├── history.jsonl -> ../shared/history.jsonl
  │   ├── CLAUDE.md     -> ../shared/CLAUDE.md
  │   ├── .claude.json   # own account identity (NOT shared)
  │   └── settings.json  # own settings
  └── personal/          # CLAUDE_CONFIG_DIR for "personal" (same shared links)
```

`ccswitch isolate` with no name lists existing isolated profiles. Existing
non-symlink files in a profile dir are left untouched (never clobbered).

**Seeding.** `shared/` starts empty — isolated profiles begin with fresh
memory. Carry your existing memory/history over once:

```sh
ccswitch seed            # copies CLAUDE.md, history.jsonl and projects/ from ~/.claude
ccswitch seed <dir>      # ... or from another config dir
```

Re-run `ccswitch seed` anytime to re-sync from the source (source wins on
same-named files). While `shared/` is empty, `ccswitch isolate` warns and asks
for confirmation before launching, so you don't start with no memory by
accident. The prompt stops once `shared/` has content.

> This pattern relies on `CLAUDE_CONFIG_DIR` (supported) plus symlinking of
> account-agnostic paths (a community pattern, not officially documented). Auth
> stays isolated; only memory/history is shared. Two sessions writing the same
> project's memory simultaneously can race — low risk in practice.

---

## Search past sessions

`ccswitch search` (alias `s`) is an optional bridge to
[`csx`](https://github.com/mysqto/csx), a local index of your AI-coding
sessions. It fuzzy-picks a session and resumes it in the tool that produced it:

```sh
ccswitch search                 # sessions for the active tool → fzf → resume
ccswitch search --repo payments # any csx scope flag passes straight through
ccswitch search --tool codex    # pick from Codex sessions instead
```

It shells out to `csx sessions --json` (defaulting the scope to the tool
reported by `csx current`), previews each transcript with `csx show <id>` in
`fzf`, and resumes the pick — `claude --resume <id>` for Claude Code, `codex
resume <id>` for Codex. Requires `csx` and `fzf` on `PATH`. If either is
missing, every other `ccswitch` command still works — this subcommand just
prints a hint.

---

## Profile storage

Profiles live in `$CCSWITCH_HOME` (default `~/.claude/accounts`), one directory
per profile:

```
~/.claude/accounts/work/
  ├── credentials.json   # OAuth blob
  └── account.json       # oauthAccount + userID (+ keychain account attr)
```

Point `$CCSWITCH_HOME` at a synced folder or repo to move profiles between
machines.

### ⚠️ These files are secrets

`credentials.json` holds a live bearer token. If you sync `$CCSWITCH_HOME` with
git or a cloud folder, **encrypt it** — use a private repo with
[git-crypt](https://github.com/AGWA/git-crypt) /
[age](https://github.com/FiloSottile/age) /
[SOPS](https://github.com/getsops/sops), or a real secret manager. Never commit
plaintext tokens to a shared or public repo.

---

## Notes

- Quit any running `claude` session before switching — an open session can
  rewrite `~/.claude.json` on exit and clobber the swap. `ccswitch` warns when
  it detects one.
- Access tokens expire, but the refresh token is restored too, so Claude Code
  re-refreshes automatically after a switch.
- OAuth refresh tokens rotate on every use. To keep snapshots valid, `ccswitch`
  re-captures the outgoing account into **every** profile that shares its token
  on each switch — so an account you actively use won't go stale.

---

## Development

```sh
cargo build
cargo test
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
```

Architecture, the ports-and-adapters layout, and the coverage discipline are
documented in [`AGENTS.md`](AGENTS.md).

## License

MIT — see [`LICENSE`](LICENSE). Copyright (c) 2026 Chen Lei.
