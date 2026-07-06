function ccswitch -d "switch between multiple Claude Code accounts"
    set -l home (__ccswitch_home)
    set -l config "$HOME/.claude.json"

    set -l cmd $argv[1]
    switch "$cmd"
        case save
            __ccswitch_save "$home" "$config" $argv[2]
        case add
            __ccswitch_add "$home" "$config" $argv[2]
        case isolate iso
            __ccswitch_isolate $argv[2..-1]
        case seed
            __ccswitch_seed $argv[2..-1]
        case search s
            __ccswitch_search $argv[2..-1]
        case list ls
            __ccswitch_list "$home" "$config"
        case current whoami
            __ccswitch_current "$config"
        case use
            __ccswitch_use "$home" "$config" $argv[2]
        case rm remove delete
            __ccswitch_rm "$home" $argv[2]
        case '' -h --help help
            __ccswitch_help
        case '*'
            # bare profile name -> switch to it, then launch a claude session
            if test -d "$home/$cmd"
                __ccswitch_use "$home" "$config" "$cmd"
                and command claude $argv[2..-1]
            else
                echo "ccswitch: unknown command or profile '$cmd'" >&2
                echo "run 'ccswitch help' for usage, 'ccswitch list' to see profiles" >&2
                return 1
            end
    end
end

function __ccswitch_home -d "resolve the profile store directory"
    if set -q CCSWITCH_HOME
        echo $CCSWITCH_HOME
    else
        echo "$HOME/.claude/accounts"
    end
end

function __ccswitch_cred_read -d "print the current Claude Code OAuth credential blob"
    switch (uname)
        case Darwin
            security find-generic-password -s "Claude Code-credentials" -w 2>/dev/null
        case '*'
            if test -f "$HOME/.claude/.credentials.json"
                cat "$HOME/.claude/.credentials.json"
            else
                return 1
            end
    end
end

function __ccswitch_cred_acct -d "print the keychain account attribute (macOS only)"
    security find-generic-password -s "Claude Code-credentials" -g 2>&1 \
        | string replace -rf '^.*"acct"<blob>="(.*)"$' '$1'
end

function __ccswitch_cred_write -d "write an OAuth credential blob into the platform store"
    set -l blob $argv[1]
    set -l acct $argv[2]
    switch (uname)
        case Darwin
            test -z "$acct"; and set acct "$USER"
            security delete-generic-password -s "Claude Code-credentials" >/dev/null 2>&1
            security add-generic-password -U -a "$acct" -s "Claude Code-credentials" -w "$blob"
        case '*'
            mkdir -p "$HOME/.claude"
            printf '%s' "$blob" >"$HOME/.claude/.credentials.json"
            chmod 600 "$HOME/.claude/.credentials.json"
    end
end

function __ccswitch_save -d "snapshot the current account into a profile"
    set -l home $argv[1]
    set -l config $argv[2]
    set -l name $argv[3]

    if test -z "$name"
        echo "usage: ccswitch save <name>" >&2
        return 1
    end
    if contains -- "$name" save add isolate iso seed search s shared list ls current whoami use rm remove delete help
        echo "ccswitch: '$name' is a reserved word, pick another profile name" >&2
        return 1
    end
    if not test -f "$config"
        echo "ccswitch: $config not found — is Claude Code installed?" >&2
        return 1
    end

    set -l blob (__ccswitch_cred_read | string collect)
    if test -z "$blob"
        echo "ccswitch: no active credential found — log in with 'claude' first" >&2
        return 1
    end

    set -l dir "$home/$name"
    mkdir -p "$dir"
    chmod 700 "$home" "$dir"

    printf '%s' "$blob" >"$dir/credentials.json"
    chmod 600 "$dir/credentials.json"

    set -l acct ""
    test (uname) = Darwin; and set acct (__ccswitch_cred_acct)
    jq --arg acct "$acct" '{oauthAccount, userID, keychain_account: $acct}' "$config" >"$dir/account.json"
    chmod 600 "$dir/account.json"

    set -l email (jq -r '.oauthAccount.emailAddress // "unknown"' "$dir/account.json")
    echo "saved profile '$name' ($email)"
end

function __ccswitch_add -d "log in a new account and save it as a profile"
    set -l home $argv[1]
    set -l config $argv[2]
    set -l name $argv[3]

    if test -z "$name"
        echo "usage: ccswitch add <name>" >&2
        return 1
    end
    if contains -- "$name" save add isolate iso seed search s shared list ls current whoami use rm remove delete help
        echo "ccswitch: '$name' is a reserved word, pick another profile name" >&2
        return 1
    end
    if test -d "$home/$name"
        echo "ccswitch: profile '$name' already exists (ccswitch rm $name to replace it)" >&2
        return 1
    end

    # note: no `claude auth logout` here — logout may revoke the token
    # server-side, which would invalidate an already-saved profile. `login`
    # simply replaces the active credential slot, which is all we need.
    echo "signing in as '$name' (a browser window will open)..."
    if not command claude auth login
        echo "ccswitch: login did not complete — nothing saved" >&2
        return 1
    end
    __ccswitch_save "$home" "$config" "$name"
end

function __ccswitch_sync_current -d "re-snapshot the live account into its matching profile"
    # OAuth refresh tokens rotate on every use, so a profile saved earlier goes
    # stale as its account keeps running. Before switching away, copy the live
    # credential + identity back into whichever profile matches the active
    # account, so its snapshot stays valid.
    set -l home $argv[1]
    set -l config $argv[2]

    test -f "$config"; or return 0
    set -l cur_key (jq -r '(.oauthAccount.accountUuid // "") + "|" + (.oauthAccount.organizationUuid // "")' "$config")
    test "$cur_key" = "|"; and return 0

    set -l blob (__ccswitch_cred_read | string collect)
    test -z "$blob"; and return 0

    test -d "$home"; or return 0
    for dir in "$home"/*/
        test -f "$dir/account.json"; or continue
        set -l key (jq -r '(.oauthAccount.accountUuid // "") + "|" + (.oauthAccount.organizationUuid // "")' "$dir/account.json")
        test "$key" = "$cur_key"; or continue
        printf '%s' "$blob" >"$dir/credentials.json"
        chmod 600 "$dir/credentials.json"
        set -l acct ""
        test (uname) = Darwin; and set acct (__ccswitch_cred_acct)
        jq --arg acct "$acct" '{oauthAccount, userID, keychain_account: $acct}' "$config" >"$dir/account.json"
        chmod 600 "$dir/account.json"
        return 0
    end
    return 0
end

function __ccswitch_use -d "restore a profile as the active account"
    set -l home $argv[1]
    set -l config $argv[2]
    set -l name $argv[3]

    if test -z "$name"
        echo "usage: ccswitch use <name>" >&2
        return 1
    end
    set -l dir "$home/$name"
    if not test -d "$dir"
        echo "ccswitch: profile '$name' not found (ccswitch list)" >&2
        return 1
    end
    if not test -f "$dir/credentials.json"; or not test -f "$dir/account.json"
        echo "ccswitch: profile '$name' is incomplete" >&2
        return 1
    end
    if not test -f "$config"
        echo "ccswitch: $config not found" >&2
        return 1
    end

    if command -sq pgrep; and pgrep -x claude >/dev/null 2>&1
        echo "ccswitch: warning — a running 'claude' may overwrite ~/.claude.json on exit; quit it first" >&2
    end

    # refresh the outgoing account's snapshot (its token may have rotated)
    __ccswitch_sync_current "$home" "$config"

    # restore the credential into the platform store
    set -l blob (cat "$dir/credentials.json" | string collect)
    set -l acct (jq -r '.keychain_account // ""' "$dir/account.json")
    if not __ccswitch_cred_write "$blob" "$acct"
        echo "ccswitch: failed to write credential" >&2
        return 1
    end

    # splice identity into ~/.claude.json (back it up first, never rewrite wholesale)
    cp "$config" "$config.ccswitch.bak"
    set -l tmp (mktemp)
    if jq --slurpfile a "$dir/account.json" \
            '.oauthAccount = $a[0].oauthAccount | .userID = $a[0].userID' \
            "$config" >"$tmp"
        mv "$tmp" "$config"
        chmod 600 "$config"
    else
        rm -f "$tmp"
        mv "$config.ccswitch.bak" "$config"
        echo "ccswitch: failed to patch $config (restored from backup)" >&2
        return 1
    end

    set -l email (jq -r '.oauthAccount.emailAddress // "unknown"' "$dir/account.json")
    echo "switched to '$name' ($email)"
end

function __ccswitch_list -d "list saved profiles"
    set -l home $argv[1]
    set -l config $argv[2]

    # a profile is identified by account *and* org (same login can span orgs)
    set -l active_key ""
    test -f "$config"; and set active_key (jq -r '(.oauthAccount.accountUuid // "") + "|" + (.oauthAccount.organizationUuid // "")' "$config")

    set -l found 0
    if test -d "$home"
        for dir in "$home"/*/
            test -f "$dir/account.json"; or continue
            set found 1
            set -l name (basename "$dir")
            set -l email (jq -r '.oauthAccount.emailAddress // "unknown"' "$dir/account.json")
            set -l org (jq -r '.oauthAccount.organizationName // ""' "$dir/account.json")
            set -l key (jq -r '(.oauthAccount.accountUuid // "") + "|" + (.oauthAccount.organizationUuid // "")' "$dir/account.json")
            set -l mark "  "
            test "$key" != "|"; and test "$key" = "$active_key"; and set mark "* "
            if test -n "$org"
                printf '%s%-16s %s (%s)\n' "$mark" "$name" "$email" "$org"
            else
                printf '%s%-16s %s\n' "$mark" "$name" "$email"
            end
        end
    end
    test $found -eq 0; and echo "no profiles yet — save one with 'ccswitch save <name>'"
end

function __ccswitch_current -d "show the active account"
    set -l config $argv[1]
    if not test -f "$config"
        echo "ccswitch: $config not found" >&2
        return 1
    end
    set -l email (jq -r '.oauthAccount.emailAddress // "unknown"' "$config")
    set -l org (jq -r '.oauthAccount.organizationName // ""' "$config")
    if test -n "$org"
        echo "$email ($org)"
    else
        echo "$email"
    end
end

function __ccswitch_rm -d "delete a saved profile"
    set -l home $argv[1]
    set -l name $argv[2]
    if test -z "$name"
        echo "usage: ccswitch rm <name>" >&2
        return 1
    end
    set -l dir "$home/$name"
    if not test -d "$dir"
        echo "ccswitch: profile '$name' not found" >&2
        return 1
    end
    rm -rf "$dir"
    echo "removed profile '$name'"
end

function __ccswitch_isolate_home -d "resolve the isolated-profile base directory"
    if set -q CCSWITCH_ISOLATE_HOME
        echo $CCSWITCH_ISOLATE_HOME
    else
        echo "$HOME/.claude/profiles"
    end
end

function __ccswitch_link -d "symlink a shared path into a profile dir, non-destructively"
    set -l target $argv[1]
    set -l link $argv[2]
    if test -L "$link"
        ln -sfn "$target" "$link"
    else if test -e "$link"
        echo "ccswitch: $link exists and is not a symlink — leaving it as-is" >&2
    else
        ln -s "$target" "$link"
    end
end

function __ccswitch_seeded -d "true if the shared isolate dir has any seeded memory/history"
    set -l shared $argv[1]
    test -s "$shared/history.jsonl"; and return 0
    test -s "$shared/CLAUDE.md"; and return 0
    if test -d "$shared/projects"
        set -l first (find "$shared/projects" -mindepth 1 -maxdepth 1 2>/dev/null | head -1)
        test -n "$first"; and return 0
    end
    return 1
end

function __ccswitch_isolate -d "launch a concurrent session isolated to a profile, memory shared"
    set -l base (__ccswitch_isolate_home)
    set -l shared "$base/shared"
    set -l name $argv[1]

    if test -z "$name"
        echo "isolated profiles in $base:"
        set -l any 0
        if test -d "$base"
            for d in "$base"/*/
                set -l n (basename "$d")
                test "$n" = shared; and continue
                test -L "$d/projects"; or continue
                set any 1
                echo "  $n"
            end
        end
        test $any -eq 0; and echo "  (none yet)"
        echo "usage: ccswitch isolate <name> [claude args...]"
        return 0
    end
    if contains -- "$name" shared save add isolate iso seed search s list ls current whoami use rm remove delete help
        echo "ccswitch: '$name' is a reserved word, pick another profile name" >&2
        return 1
    end

    set -l dir "$base/$name"
    mkdir -p "$shared/projects" "$dir"
    touch "$shared/history.jsonl"
    test -e "$shared/CLAUDE.md"; or touch "$shared/CLAUDE.md"

    # share memory/history across profiles, keep auth (.claude.json + creds) per-dir
    __ccswitch_link "$shared/projects" "$dir/projects"
    __ccswitch_link "$shared/history.jsonl" "$dir/history.jsonl"
    __ccswitch_link "$shared/CLAUDE.md" "$dir/CLAUDE.md"

    # blocking warning while the shared memory is still empty (unseeded)
    if not __ccswitch_seeded "$shared"
        echo "⚠  shared isolate memory is empty — this session starts with no" >&2
        echo "   history or CLAUDE.md. Run 'ccswitch seed' first to import your" >&2
        echo "   ~/.claude memory/history." >&2
        read -l -P "   launch '$name' anyway? [y/N] " ans
        if not string match -rqi '^y(es)?$' -- "$ans"
            echo "aborted — nothing launched" >&2
            return 1
        end
    end

    echo "launching isolated '$name' — CLAUDE_CONFIG_DIR=$dir (memory shared via $shared)"
    echo "(first run for a profile will ask you to sign in)"
    env CLAUDE_CONFIG_DIR="$dir" claude $argv[2..-1]
end

function __ccswitch_seed -d "sync the shared isolate memory/history from ~/.claude (or a given dir)"
    set -l base (__ccswitch_isolate_home)
    set -l shared "$base/shared"
    set -l src $argv[1]
    test -z "$src"; and set src "$HOME/.claude"

    if not test -d "$src"
        echo "ccswitch: source '$src' not found" >&2
        return 1
    end

    echo "seeding shared memory in $shared from $src ..."
    mkdir -p "$shared/projects"
    set -l did 0
    if test -f "$src/CLAUDE.md"
        cp "$src/CLAUDE.md" "$shared/CLAUDE.md"
        set did 1
        echo "  CLAUDE.md"
    end
    if test -f "$src/history.jsonl"
        cp "$src/history.jsonl" "$shared/history.jsonl"
        set did 1
        echo "  history.jsonl"
    end
    if test -d "$src/projects"
        cp -R "$src/projects/." "$shared/projects/"
        set did 1
        echo "  projects/ (transcripts + memory)"
    end
    if test $did -eq 0
        echo "  nothing to seed from $src"
        return 0
    end
    echo "done — isolate profiles now share this memory/history"
end

function __ccswitch_search -d "fuzzy-pick a past session via csx and resume it"
    if not type -q csx
        echo "ccswitch: csx not found on PATH — see github.com/mysqto/csx" >&2
        return 127
    end
    if not type -q fzf
        echo "ccswitch: fzf not found on PATH" >&2
        return 127
    end

    # default the scope to the active tool unless the caller passed --tool
    set -l tool
    if contains -- --tool $argv
        for i in (seq (count $argv))
            test "$argv[$i]" = --tool; and set tool $argv[(math $i + 1)]
        end
    else if type -q jq
        set tool (csx current --json 2>/dev/null | jq -r '.[0].tool // empty' 2>/dev/null)
    end

    set -l scope $argv
    if not contains -- --tool $argv; and test -n "$tool"
        set scope --tool $tool $scope
    end

    # project each session to "id<TAB>label"; fzf shows the label, keys off the id
    set -l rows
    if type -q jq
        set rows (csx sessions --json $scope 2>/dev/null | jq -r '.[] | "\(.session_id)\t\(.tool // "-")  \(.project_name // "-")  \(.git_branch // "-")  (\(.msg_count) msgs)"')
    else
        set rows (csx sessions $scope 2>/dev/null | string match -r -v '^(LAST|no )')
    end
    if test -z "$rows"
        echo "ccswitch: no sessions matched" >&2
        return 1
    end

    set -l picked (printf '%s\n' $rows | fzf --with-nth=2.. --delimiter='\t' \
        --preview 'csx show {1}' --preview-window='right,60%,wrap' --prompt='session> ')
    or return $status
    set -l id (printf '%s' $picked | string split -f1 \t)
    test -z "$id"; and return 1

    __ccswitch_resume "$tool" "$id"
end

function __ccswitch_resume --argument-names tool id -d "resume a session with its originating tool"
    switch "$tool"
        case claude-code ''
            command claude --resume $id
        case codex
            command codex resume $id
        case '*'
            echo "ccswitch: don't know how to resume tool '$tool' (session $id)" >&2
            return 2
    end
end

function __ccswitch_help -d "show ccswitch usage"
    echo "ccswitch — switch between multiple Claude Code accounts"
    echo
    echo "usage:"
    echo "  ccswitch <name> [args...]  switch to <name> and start a claude session"
    echo "  ccswitch use <name>        switch to <name> without launching claude"
    echo "  ccswitch add <name>        sign in to a new account and save it as <name>"
    echo "  ccswitch save <name>       save the current account as <name>"
    echo "  ccswitch isolate <name>    run a concurrent session isolated to <name>,"
    echo "                             with memory/history shared across profiles"
    echo "  ccswitch seed [dir]        copy CLAUDE.md/history/projects from ~/.claude"
    echo "                             (or [dir]) into the shared isolate memory"
    echo "  ccswitch search | s [scope]  fuzzy-pick a past session (via csx) and resume it"
    echo "  ccswitch list | ls         list saved profiles (* marks the active one)"
    echo "  ccswitch current | whoami  show the active account"
    echo "  ccswitch rm <name>         delete a saved profile"
    echo "  ccswitch help              show this help"
    echo
    echo "profiles live in \$CCSWITCH_HOME (default ~/.claude/accounts)."
    echo "isolated profiles live in \$CCSWITCH_ISOLATE_HOME (default ~/.claude/profiles)."
end
