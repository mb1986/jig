#compdef jig

_jig() {
    typeset -A opt_args
    local context curcontext="$curcontext" state state_descr line
    local ret=1
    local -a config_args

    # Capture an explicit `--config` from the words on the command
    # line so dynamic candidate calls reflect the user-chosen config.
    local i word
    for (( i = 2; i < CURRENT; i++ )); do
        word="${words[$i]}"
        if [[ "$word" == "--config" && $((i + 1)) -lt CURRENT ]]; then
            config_args=("--config" "${words[$((i + 1))]}")
            break
        fi
        if [[ "$word" == --config=* ]]; then
            config_args=("$word")
            break
        fi
    done

    _arguments -s -S -C \
        '(-h --help)-h[Print help]' \
        '(-h --help)--help[Print help]' \
        '(-V --version)-V[Print version]' \
        '(-V --version)--version[Print version]' \
        '(-l --list)-l[List configured commands and profiles]' \
        '(-l --list)--list[List configured commands and profiles]' \
        '(-n --dry-run)-n[Print the resolved command without running it]' \
        '(-n --dry-run)--dry-run[Print the resolved command without running it]' \
        '--config=[Use this config file]:path:_files' \
        '1:command:->command' \
        '2:profile:->profile' \
        '*::passthrough:_files' \
        && ret=0

    case $state in
        command)
            local -a candidates
            candidates=("${(@f)$("${words[1]}" "${config_args[@]}" --list-commands 2>/dev/null)}")
            _describe -t commands 'command or alias' candidates && ret=0
            ;;
        profile)
            local cmd="${words[2]}"
            local -a candidates
            candidates=("${(@f)$("${words[1]}" "${config_args[@]}" --list-profiles "$cmd" 2>/dev/null)}")
            _describe -t profiles 'profile' candidates && ret=0
            ;;
    esac

    return ret
}

_jig "$@"
