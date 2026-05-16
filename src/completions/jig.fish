# Tokens already on the command line, excluding the partial token
# being typed. Element 1 is `jig` itself.
function __jig_args
    commandline -opc
end

# Expand a leading `~/` or bare `~` in $argv[1] to $HOME. A captured
# token (e.g. the value typed after `--config`) keeps `~` literal —
# shells only expand `~` at parse time on unquoted words. Without
# this, `jig --config ~/x.kdl --list-commands` fails to find the file
# and completion silently returns no candidates. `~user/...` is left
# alone — uncommon in practice and would require getent/dscacheutil.
function __jig_expand_tilde
    string replace -r '^~(?=/|$)' "$HOME" -- $argv[1]
end

# Echo `--config <PATH>` (or `--config=<PATH>`) extracted from the
# current command line, so dynamic candidate calls reflect the
# user-chosen config. Echoes nothing if the user did not pass one.
function __jig_config_args
    set -l toks (__jig_args)
    set -l n (count $toks)
    set -l i 2
    while test $i -le $n
        set -l t $toks[$i]
        if test "$t" = "--config"
            set -l next (math $i + 1)
            if test $next -le $n
                echo "--config"
                __jig_expand_tilde $toks[$next]
            end
            return
        end
        if string match -q -- "--config=*" $t
            echo "--config="(__jig_expand_tilde (string replace -- "--config=" "" $t))
            return
        end
        set i (math $i + 1)
    end
end

# Echo each positional token already on the command line, one per
# line. Mirrors jig's argv split (src/cli.rs `split_argv`): jig
# flags are only meaningful before the first positional, so once a
# non-flag token has appeared every later token — `-x` included —
# is command/profile/pass-through context.
function __jig_positionals_in_argv
    set -l toks (__jig_args)
    set -l n (count $toks)
    set -l skip 0
    set -l scanning 1
    set -l i 2
    while test $i -le $n
        set -l t $toks[$i]
        if test $scanning -eq 1
            if test $skip -eq 1
                set skip 0
                set i (math $i + 1)
                continue
            end
            if test "$t" = "--"
                set scanning 0
                set i (math $i + 1)
                continue
            end
            if test "$t" = "--config" -o "$t" = "--list-profiles" -o "$t" = "--completions"
                set skip 1
                set i (math $i + 1)
                continue
            end
            if string match -q -- "--config=*" $t
                set i (math $i + 1)
                continue
            end
            if string match -q -- "-*" $t
                set i (math $i + 1)
                continue
            end
            set scanning 0
        end
        echo $t
        set i (math $i + 1)
    end
end

# Number of positionals already typed. 0 = command/alias,
# 1 = profile, 2+ = pass-through.
function __jig_positional
    count (__jig_positionals_in_argv)
end

# Echo the first positional token already typed (the command/alias),
# or nothing if there is none yet.
function __jig_first_positional
    __jig_positionals_in_argv | head -n 1
end

# True when a "print-and-exit" jig flag is already on the command
# line. In those modes (`--list`, `--cat`, `--completions`) no command
# name is needed, so command/profile candidates are suppressed.
# `--explain` and `--dry-run` are deliberately omitted: they operate
# on a command, so completion stays useful.
function __jig_terminal_flag_seen
    __fish_seen_argument -s l -l list -l cat -l completions
end

function __jig_complete_commands
    jig (__jig_config_args) --list-commands 2>/dev/null
end

function __jig_complete_profiles
    set -l cmd (__jig_first_positional)
    if test -n "$cmd"
        jig (__jig_config_args) --list-profiles "$cmd" 2>/dev/null
    end
end

# Disable default file completion so positional dispatch is clean.
complete -c jig -f
# Each flag is suppressed once it (or a flag it conflicts with) is
# already on the command line. Conflict graph mirrors clap's
# `conflicts_with_all` declarations in `src/cli.rs`; both sides are
# listed so the relation is symmetric.
complete -c jig -n 'not __fish_seen_argument -s h -l help' \
    -s h -l help -d 'Print help'
complete -c jig -n 'not __fish_seen_argument -s V -l version' \
    -s V -l version -d 'Print version'
complete -c jig -n 'not __fish_seen_argument -s l -l list -l cat -s x -l explain' \
    -s l -l list -d 'List configured commands and profiles'
complete -c jig -n 'not __fish_seen_argument -s n -l dry-run -l cat -s x -l explain' \
    -s n -l dry-run -d 'Print the resolved command without running it'
complete -c jig -n 'not __fish_seen_argument -s x -l explain -s l -l list -s n -l dry-run -l cat -l completions' \
    -s x -l explain -d 'Trace how the resolved command was assembled'
complete -c jig -n 'not __fish_seen_argument -l cat -s l -l list -s n -l dry-run -s x -l explain -l completions' \
    -l cat -d 'Dump the loaded config file to stdout'
complete -c jig -n 'not __fish_seen_argument -s q -l quiet' \
    -s q -l quiet -d 'Suppress the pre-exec preview line'
# Value-taking jig flags only make sense before the first positional —
# after that, they're pass-through tokens to the child command and the
# following value belongs to the child, not jig. We deliberately do
# NOT self-block these with `__fish_seen_argument -l config` /
# `-l completions`: the same `-n` gate controls the option's value
# candidates (e.g. `zsh bash fish` after `--completions `), so a
# self-block would suppress the value list once the option is typed.
complete -c jig -n 'test (__jig_positional) -eq 0' \
    -l config -d 'Use this config file' -r -F
complete -c jig -n 'test (__jig_positional) -eq 0; and not __fish_seen_argument -l cat -s x -l explain' \
    -l completions -d 'Print a shell completion script' -x -a 'zsh bash fish'

complete -c jig -n 'test (__jig_positional) -eq 0; and not __jig_terminal_flag_seen' \
    -a '(__jig_complete_commands)' -d 'command or alias'
complete -c jig -n 'test (__jig_positional) -eq 1; and not __jig_terminal_flag_seen' \
    -a '(__jig_complete_profiles)' -d 'profile'
complete -c jig -n 'test (__jig_positional) -ge 2' -F
