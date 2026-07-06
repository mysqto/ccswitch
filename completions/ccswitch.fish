function __ccswitch_profiles -d "list saved ccswitch profile names"
    set -l home
    if set -q CCSWITCH_HOME
        set home $CCSWITCH_HOME
    else
        set home "$HOME/.claude/accounts"
    end
    test -d "$home"; or return
    for dir in "$home"/*/
        test -f "$dir/account.json"; and basename "$dir"
    end
end

complete -c ccswitch -f

# subcommands and bare profile names (first token only)
complete -c ccswitch -n __fish_use_subcommand -a add -d "log in a new account and save it"
complete -c ccswitch -n __fish_use_subcommand -a isolate -d "concurrent isolated session (shared memory)"
complete -c ccswitch -n __fish_use_subcommand -a seed -d "sync shared isolate memory from ~/.claude"
complete -c ccswitch -n __fish_use_subcommand -a search -d "fuzzy-pick + resume a past session (via csx)"
complete -c ccswitch -n __fish_use_subcommand -a save -d "save current account"
complete -c ccswitch -n __fish_use_subcommand -a use -d "switch without launching"
complete -c ccswitch -n __fish_use_subcommand -a list -d "list profiles"
complete -c ccswitch -n __fish_use_subcommand -a current -d "show active account"
complete -c ccswitch -n __fish_use_subcommand -a rm -d "delete a profile"
complete -c ccswitch -n __fish_use_subcommand -a help -d "show help"
complete -c ccswitch -n __fish_use_subcommand -a "(__ccswitch_profiles)" -d "switch + launch"

# profile names as the argument to use/rm
complete -c ccswitch -n "__fish_seen_subcommand_from use rm remove delete" -a "(__ccswitch_profiles)" -d profile

function __ccswitch_iso_profiles -d "list isolated ccswitch profile names"
    set -l base
    if set -q CCSWITCH_ISOLATE_HOME
        set base $CCSWITCH_ISOLATE_HOME
    else
        set base "$HOME/.claude/profiles"
    end
    test -d "$base"; or return
    for d in "$base"/*/
        set -l n (basename "$d")
        test "$n" = shared; and continue
        test -L "$d/projects"; and echo $n
    end
end

complete -c ccswitch -n "__fish_seen_subcommand_from isolate iso" -a "(__ccswitch_iso_profiles)" -d "isolated profile"
