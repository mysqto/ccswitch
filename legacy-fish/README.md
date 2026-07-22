# ccswitch

A tiny [fish](https://fishshell.com) plugin to switch between multiple
[Claude Code](https://claude.com/claude-code) accounts — personal, work,
different orgs — and jump straight into a session.

```fish
ccswitch work          # switch to the "work" account and start claude
ccswitch personal -c   # switch to "personal", launch claude with -c
ccswitch use work      # just switch, don't launch
ccswitch list          # see every saved profile (* = active)
```

## How it works

An account is two things that must travel together:

1. **OAuth tokens** — the real credential. On macOS these live in the login
   Keychain (`Claude Code-credentials`); on Linux/others in
   `~/.claude/.credentials.json`. `ccswitch` picks the right backend per platform.
2. **Identity** — the `oauthAccount` object and `userID` inside `~/.claude.json`,
   which Claude Code validates the token against.

`ccswitch save <name>` snapshots both into a plain profile directory. Switching
restores the credential into the platform store and splices only `oauthAccount`
and `userID` back into `~/.claude.json` (backing it up first — the rest of that
file is shared and left untouched). Because a profile is just two JSON files, it
is portable across machines and operating systems.

## Install

With [fisher](https://github.com/jorgebucaran/fisher):

```fish
fisher install mysqto/ccswitch
```

Requires [`jq`](https://jqlang.github.io/jq/).

## Usage

| Command | Action |
| --- | --- |
| `ccswitch <name> [args...]` | Switch to `<name>`, then start `claude` (extra args passed through) |
| `ccswitch use <name>` | Switch without launching |
| `ccswitch add <name>` | Sign in to a new account (`claude auth login`) and save it as `<name>` |
| `ccswitch isolate <name> [args...]` | Run a **concurrent** session isolated to `<name>` (own credential) with memory/history shared across profiles |
| `ccswitch save <name>` | Save the current account as `<name>` |
| `ccswitch list` / `ls` | List profiles (`*` marks the active one) |
| `ccswitch current` / `whoami` | Show the active account |
| `ccswitch search` / `s` `[scope…]` | Fuzzy-pick a past session (via [`csx`](https://github.com/mysqto/csx)) and resume it |
| `ccswitch rm <name>` | Delete a profile |

## Search past sessions

`ccswitch search` (alias `s`) is an optional bridge to
[`csx`](https://github.com/mysqto/csx), a local index of your AI-coding
sessions. It fuzzy-picks a session and resumes it in the tool that produced it:

```fish
ccswitch search                 # sessions for the active tool → fzf → resume
ccswitch search --repo payments # any csx scope flag passes straight through
```

It shells out to `csx sessions` (defaulting the scope to the active tool),
previews each transcript with `csx show <id>` in `fzf`, and resumes the pick
(`claude --resume <id>`, `codex resume <id>`, …). Requires `csx` and `fzf` on
`PATH` (`jq` optional, for a nicer picker list). If `csx` isn't installed, every
other `ccswitch` command still works — this subcommand just prints a hint.

## Walkthrough

A complete run, from a fresh install to switching daily.

### 1. Save the account you're already signed into

```fish
claude auth status        # confirm who you're logged in as
ccswitch save personal    # snapshot the active account as "personal"
```

```fish
ccswitch list
# * personal          you@gmail.com
```

The `*` marks the currently active account.

### 2. Add a second account

`ccswitch add` signs you in to another account and saves it in one step:

```fish
ccswitch add work         # runs `claude auth login`, then saves it as "work"
```

<details>
<summary>Prefer to do it by hand?</summary>

```fish
claude auth login         # sign in as the other account (or switch org via /login)
ccswitch save work
```
</details>

Both are now saved:

```fish
ccswitch list
#   personal          you@gmail.com
# * work              you@company.com (Acme)
```

### 3. Switch and work — the daily loop

```fish
ccswitch work             # become "work" and start a claude session
ccswitch personal -c      # become "personal", continue the last conversation
ccswitch use work         # just switch, don't launch anything
ccswitch current          # you@company.com (Acme)
```

One command becomes an account and drops you into a session. Switching also
re-snapshots the account you're leaving, so rotating tokens never go stale.

### 4. Run two accounts at the same time

Steps 1–3 swap one machine-global account (sequential — one at a time). To keep
**two live sessions as different accounts concurrently**, use isolated profiles
with shared memory:

```fish
ccswitch seed             # once: copy your ~/.claude memory + history into shared/
ccswitch isolate work     # terminal 1 — sign in once as work
ccswitch isolate personal # terminal 2 — sign in once as personal, at the same time
```

Each isolated session has its own credential but shares project memory and
history. Re-running `ccswitch isolate <name>` reuses the earlier login. (If you
skip `seed`, the first `isolate` warns that memory is empty and asks to confirm.)

## Same login, multiple orgs

If one login (e.g. a Claude Team account) belongs to several organizations,
each org is a separate profile — Claude Code issues a distinct org-scoped token
per org. Save one after switching org inside Claude Code (`/login`):

```fish
ccswitch save org-a      # while the first org is active
# switch org in Claude Code, then:
ccswitch save org-b
```

`ccswitch list` distinguishes them by account **and** organization, so the same
email can appear more than once with only the active org starred.

## Concurrent sessions (shared memory, separate accounts)

`ccswitch <name>` swaps a single machine-global account, so it is sequential —
one active account at a time. To run **two accounts at once** (two terminals),
use `ccswitch isolate`, which gives each profile its own
[`CLAUDE_CONFIG_DIR`](https://code.claude.com/docs/en/settings):

```fish
ccswitch isolate work        # terminal 1 — signs in once, then reuses
ccswitch isolate personal    # terminal 2 — a different account, at the same time
```

Each profile keeps its **own credential and identity**, but memory and history
are **shared** — `isolate` symlinks `projects/` (session transcripts + the
`memory/` files), `history.jsonl`, and `CLAUDE.md` from each profile to a common
`shared/` directory. So both accounts see the same project memory and past
sessions while staying logged in as different users.

Layout (centralized under `~/.claude`, override with `$CCSWITCH_ISOLATE_HOME`):

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

`shared/` starts **empty** — isolated profiles begin with fresh memory. To carry
your existing memory/history over, seed it once from your default `~/.claude`:

```fish
ccswitch seed            # copies CLAUDE.md, history.jsonl and projects/ from ~/.claude
ccswitch seed <dir>      # ... or from another config dir
```

Re-run `ccswitch seed` anytime to re-sync from the source (source wins on
same-named files).

While `shared/` is empty, `ccswitch isolate` warns and asks for confirmation
before launching (so you don't start with no memory by accident). The prompt
stops appearing once `shared/` has content — from `ccswitch seed` or from your
first session.

> Note: this pattern relies on `CLAUDE_CONFIG_DIR` (supported) plus symlinking of
> account-agnostic paths (a community pattern, not officially documented). Auth
> stays isolated; only memory/history is shared. Two sessions writing the same
> project's memory simultaneously can race — low risk in practice.

## Profile storage

Profiles live in `$CCSWITCH_HOME` (default `~/.claude/accounts`), one directory
per profile:

```
~/.claude/accounts/work/
  ├── credentials.json   # OAuth blob
  └── account.json       # oauthAccount + userID
```

Point `$CCSWITCH_HOME` at a synced folder or repo to move profiles between
machines:

```fish
set -Ux CCSWITCH_HOME ~/vault/claude-accounts
```

## ⚠️ These files are secrets

`credentials.json` holds a live bearer token. If you sync `$CCSWITCH_HOME` with
git or a cloud folder, **encrypt it** — use a private repo with
[git-crypt](https://github.com/AGWA/git-crypt) / [age](https://github.com/FiloSottile/age) /
[SOPS](https://github.com/getsops/sops), or a real secret manager. Never commit
plaintext tokens to a shared or public repo.

## Notes

- Quit any running `claude` session before switching — an open session can
  rewrite `~/.claude.json` on exit and clobber the swap. `ccswitch` warns when it
  detects one.
- Access tokens expire, but the refresh token is restored too, so Claude Code
  re-refreshes automatically after a switch.
- OAuth refresh tokens rotate on every use. To keep snapshots valid, `ccswitch`
  re-captures the outgoing profile's own credential automatically on every
  switch — so an account you actively use won't go stale.
- **Same login, multiple orgs:** a Claude Code OAuth token is bound
  server-side to the organization it was minted under, so each org profile
  needs its own token. Run `claude auth login` and pick the org, then
  `ccswitch save <name>` — this tool keys profiles by `(account, org)` and
  never copies one org's token onto another.

## License

MIT
