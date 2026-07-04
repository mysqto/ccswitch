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
| `ccswitch save <name>` | Save the current account as `<name>` |
| `ccswitch list` / `ls` | List profiles (`*` marks the active one) |
| `ccswitch current` / `whoami` | Show the active account |
| `ccswitch rm <name>` | Delete a profile |

Typical first run:

```fish
claude                 # log in as your first account
ccswitch save personal
claude /logout         # or log out via the app, then log in as the other account
ccswitch save work
ccswitch work          # from now on, switch freely
```

### Same login, multiple orgs

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

## License

MIT
