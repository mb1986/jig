_jig() {
    local cur="${COMP_WORDS[COMP_CWORD]}"
    local prev="${COMP_WORDS[COMP_CWORD-1]}"
    local i

    # Filename completion when the cursor sits on `--config`'s value.
    if [[ "$prev" == "--config" ]]; then
        COMPREPLY=( $(compgen -f -- "$cur") )
        return
    fi

    # Mirror jig's argv split: parse jig flags only before the first
    # positional command. After that, every token is command/profile/
    # pass-through context, even if it starts with `-`.
    local config_args=()
    local positionals=()
    local skip_next=0
    local scanning_flags=1
    for (( i = 1; i < COMP_CWORD; i++ )); do
        local w="${COMP_WORDS[$i]}"
        if (( scanning_flags )); then
            if (( skip_next )); then
                skip_next=0
                continue
            fi
            case "$w" in
                --)
                    scanning_flags=0
                    continue
                    ;;
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
            scanning_flags=0
        fi
        positionals+=("$w")
    done

    # Hyphen-prefixed cursor word: only complete jig's own flags when
    # no positional has been entered yet. Past the first positional,
    # `-` is pass-through, so fall through to file completion.
    if [[ "$cur" == -* ]]; then
        if (( ${#positionals[@]} == 0 )); then
            COMPREPLY=( $(compgen -W "-h --help -V --version -l --list -n --dry-run --config" -- "$cur") )
        else
            COMPREPLY=( $(compgen -f -- "$cur") )
        fi
        return
    fi

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
