function ccswitch -d "switch between multiple Claude Code accounts"
    set -l home (__ccswitch_home)
    set -l config "$HOME/.claude.json"

    set -l cmd $argv[1]
    switch "$cmd"
        case save
            __ccswitch_save "$home" "$config" $argv[2]
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
    if contains -- "$name" save list ls current whoami use rm remove delete help
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

function __ccswitch_help -d "show ccswitch usage"
    echo "ccswitch — switch between multiple Claude Code accounts"
    echo
    echo "usage:"
    echo "  ccswitch <name> [args...]  switch to <name> and start a claude session"
    echo "  ccswitch use <name>        switch to <name> without launching claude"
    echo "  ccswitch save <name>       save the current account as <name>"
    echo "  ccswitch list | ls         list saved profiles (* marks the active one)"
    echo "  ccswitch current | whoami  show the active account"
    echo "  ccswitch rm <name>         delete a saved profile"
    echo "  ccswitch help              show this help"
    echo
    echo "profiles live in \$CCSWITCH_HOME (default ~/.claude/accounts)."
end
