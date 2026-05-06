# Tokens already on the command line, excluding the partial token
# being typed. Element 1 is `jig` itself.
function __jig_args
    commandline -opc
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
                echo "$toks[$next]"
            end
            return
        end
        if string match -q -- "--config=*" $t
            echo $t
            return
        end
        set i (math $i + 1)
    end
end

# Number of positionals already typed (skipping flags and their
# values). 0 = command/alias, 1 = profile, 2+ = pass-through.
function __jig_positional
    set -l toks (__jig_args)
    set -l n (count $toks)
    set -l skip 0
    set -l count 0
    set -l i 2
    while test $i -le $n
        set -l t $toks[$i]
        if test $skip -eq 1
            set skip 0
        else if test "$t" = "--config" -o "$t" = "--list-profiles" -o "$t" = "--completions"
            set skip 1
        else if string match -q -- "-*" $t
            # bare flag, no value
        else
            set count (math $count + 1)
        end
        set i (math $i + 1)
    end
    echo $count
end

# Echo the first positional token already typed (the command/alias),
# or nothing if there is none yet.
function __jig_first_positional
    set -l toks (__jig_args)
    set -l n (count $toks)
    set -l skip 0
    set -l i 2
    while test $i -le $n
        set -l t $toks[$i]
        if test $skip -eq 1
            set skip 0
        else if test "$t" = "--config" -o "$t" = "--list-profiles" -o "$t" = "--completions"
            set skip 1
        else if string match -q -- "-*" $t
            # bare flag, no value
        else
            echo $t
            return
        end
        set i (math $i + 1)
    end
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
complete -c jig -s h -l help -d 'Print help'
complete -c jig -s V -l version -d 'Print version'
complete -c jig -s l -l list -d 'List configured commands and profiles'
complete -c jig -s n -l dry-run -d 'Print the resolved command without running it'
complete -c jig -l config -d 'Use this config file' -r -F

complete -c jig -n 'test (__jig_positional) -eq 0' -a '(__jig_complete_commands)' -d 'command or alias'
complete -c jig -n 'test (__jig_positional) -eq 1' -a '(__jig_complete_profiles)' -d 'profile'
complete -c jig -n 'test (__jig_positional) -ge 2' -F
