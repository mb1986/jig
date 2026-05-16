# Expand a leading `~/` or bare `~` in $1 to $HOME. A captured token
# (e.g. the value typed after `--config`) keeps `~` literal because
# shells only expand `~` at parse time on unquoted words. Without this,
# `jig --config ~/x.kdl --list-commands` fails to find the file and
# completion silently returns no candidates. `~user/...` is left
# alone — uncommon in practice and would require getent/dscacheutil.
_jig_expand_tilde() {
    case "$1" in
        "~")   printf '%s' "$HOME" ;;
        "~/"*) printf '%s' "$HOME/${1#"~/"}" ;;
        *)     printf '%s' "$1" ;;
    esac
}

_jig() {
    local cur="${COMP_WORDS[COMP_CWORD]}"
    local prev="${COMP_WORDS[COMP_CWORD-1]}"
    local i

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
                        local expanded
                        expanded=$(_jig_expand_tilde "${COMP_WORDS[$((i + 1))]}")
                        config_args=("--config" "$expanded")
                    fi
                    skip_next=1
                    continue
                    ;;
                --config=*)
                    local expanded
                    expanded=$(_jig_expand_tilde "${w#--config=}")
                    config_args=("--config=$expanded")
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

    # Value completion for jig's own flags is only meaningful before
    # the first positional. After that, `--config` / `--completions`
    # are pass-through tokens to the child command — what follows is
    # the child's argument, not jig's, so we fall through to generic
    # file completion below.
    if (( ${#positionals[@]} == 0 )); then
        if [[ "$prev" == "--config" ]]; then
            COMPREPLY=( $(compgen -f -- "$cur") )
            return
        fi
        if [[ "$prev" == "--completions" ]]; then
            COMPREPLY=( $(compgen -W "zsh bash fish" -- "$cur") )
            return
        fi
    fi

    # Hyphen-prefixed cursor word: only complete jig's own flags when
    # no positional has been entered yet. Past the first positional,
    # `-` is pass-through, so fall through to file completion.
    if [[ "$cur" == -* ]]; then
        if (( ${#positionals[@]} == 0 )); then
            COMPREPLY=( $(compgen -W "-h --help -V --version -l --list -n --dry-run -x --explain --cat -q --quiet --config --completions" -- "$cur") )
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
