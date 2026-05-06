#compdef jig

_jig() {
    local curcontext="$curcontext" state line
    typeset -A opt_args

    # Capture an explicit `--config` from the words on the command
    # line so dynamic candidate calls reflect the user-chosen config.
    local -a config_args
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

    _arguments -C \
        '(-h --help)'{-h,--help}'[Print help]' \
        '(-V --version)'{-V,--version}'[Print version]' \
        '(-l --list)'{-l,--list}'[List configured commands and profiles]' \
        '(-n --dry-run)'{-n,--dry-run}'[Print the resolved command without running it]' \
        '--config=[Use this config file]:path:_files' \
        '1: :->command' \
        '2: :->profile' \
        '*::passthrough:_files'

    case $state in
        command)
            local -a candidates
            candidates=("${(@f)$("${words[1]}" "${config_args[@]}" --list-commands 2>/dev/null)}")
            _describe 'command or alias' candidates
            ;;
        profile)
            local cmd="${words[2]}"
            local -a candidates
            candidates=("${(@f)$("${words[1]}" "${config_args[@]}" --list-profiles "$cmd" 2>/dev/null)}")
            _describe 'profile' candidates
            ;;
    esac
}

_jig "$@"
