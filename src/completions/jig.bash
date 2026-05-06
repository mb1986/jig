_jig() {
    local cur="${COMP_WORDS[COMP_CWORD]}"
    local prev="${COMP_WORDS[COMP_CWORD-1]}"
    local i

    # Filename completion when the cursor sits on `--config`'s value.
    if [[ "$prev" == "--config" ]]; then
        COMPREPLY=( $(compgen -f -- "$cur") )
        return
    fi

    # When the user is typing a flag, complete jig's own flags.
    if [[ "$cur" == -* ]]; then
        COMPREPLY=( $(compgen -W "-h --help -V --version -l --list -n --dry-run --config" -- "$cur") )
        return
    fi

    # Walk the words preceding the cursor: forward an explicit
    # `--config` to candidate calls and collect positional tokens
    # in source order, skipping flags and their values.
    local config_args=()
    local positionals=()
    local skip_next=0
    for (( i = 1; i < COMP_CWORD; i++ )); do
        local w="${COMP_WORDS[$i]}"
        if (( skip_next )); then
            skip_next=0
            continue
        fi
        case "$w" in
            --config)
                if (( i + 1 < COMP_CWORD )); then
                    config_args=("--config" "${COMP_WORDS[$((i + 1))]}")
                fi
                skip_next=1
                continue
                ;;
            --config=*)
                config_args=("$w")
                continue
                ;;
            --list-profiles|--completions)
                skip_next=1
                continue
                ;;
            -*)
                continue
                ;;
        esac
        positionals+=("$w")
    done

    case ${#positionals[@]} in
        0)
            local out
            out=$("${COMP_WORDS[0]}" "${config_args[@]}" --list-commands 2>/dev/null)
            COMPREPLY=( $(compgen -W "$out" -- "$cur") )
            ;;
        1)
            local out
            out=$("${COMP_WORDS[0]}" "${config_args[@]}" --list-profiles "${positionals[0]}" 2>/dev/null)
            COMPREPLY=( $(compgen -W "$out" -- "$cur") )
            ;;
        *)
            COMPREPLY=( $(compgen -f -- "$cur") )
            ;;
    esac
}
complete -F _jig jig
