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

# Echo the space-separated set of jig flags to offer as `-<TAB>`
# candidates, with already-typed flags and their conflicts removed.
# Arguments are the flag tokens already seen on the command line.
# The conflict graph mirrors clap's `conflicts_with_all` declarations
# in `src/cli.rs`; listing each side in the other's blocked-set makes
# it symmetric. Avoids associative arrays so the script stays
# bash 3.2-compatible (macOS default).
_jig_filter_flags() {
    local all="-h --help -V --version -l --list -n --dry-run -x --explain --cat -q --quiet --config --completions"
    local blocked=" "
    local arg
    for arg in "$@"; do
        case "$arg" in
            -h|--help)
                blocked+="-h --help " ;;
            -V|--version)
                blocked+="-V --version " ;;
            -l|--list)
                blocked+="-l --list --cat -x --explain " ;;
            -n|--dry-run)
                blocked+="-n --dry-run --cat -x --explain " ;;
            -x|--explain)
                blocked+="-x --explain -l --list -n --dry-run --cat --completions " ;;
            --cat)
                blocked+="--cat -l --list -n --dry-run -x --explain --completions " ;;
            -q|--quiet)
                blocked+="-q --quiet " ;;
            --config|--config=*)
                blocked+="--config " ;;
            --completions|--completions=*)
                blocked+="--completions --cat -x --explain " ;;
        esac
    done
    local out=""
    local flag
    for flag in $all; do
        case "$blocked" in
            *" $flag "*) ;;
            *) out+="$flag " ;;
        esac
    done
    printf '%s' "$out"
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
    local seen_flags=()
    # Set to 1 once any "print-and-exit" jig flag (`--list`, `-l`,
    # `--cat`, `--completions`) has been seen. In those modes no
    # command name is needed, so dynamic command/profile candidates
    # are suppressed below.
    local terminal_flag_seen=0
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
                    seen_flags+=("--config")
                    skip_next=1
                    continue
                    ;;
                --config=*)
                    local expanded
                    expanded=$(_jig_expand_tilde "${w#--config=}")
                    config_args=("--config=$expanded")
                    seen_flags+=("--config")
                    continue
                    ;;
                --list-profiles)
                    skip_next=1
                    continue
                    ;;
                --completions)
                    seen_flags+=("--completions")
                    terminal_flag_seen=1
                    skip_next=1
                    continue
                    ;;
                --completions=*)
                    seen_flags+=("--completions")
                    terminal_flag_seen=1
                    continue
                    ;;
                -l|--list|--cat)
                    seen_flags+=("$w")
                    terminal_flag_seen=1
                    continue
                    ;;
                -*)
                    seen_flags+=("$w")
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
            local candidates
            if (( ${#seen_flags[@]} > 0 )); then
                candidates=$(_jig_filter_flags "${seen_flags[@]}")
            else
                candidates=$(_jig_filter_flags)
            fi
            COMPREPLY=( $(compgen -W "$candidates" -- "$cur") )
        else
            COMPREPLY=( $(compgen -f -- "$cur") )
        fi
        return
    fi

    case ${#positionals[@]} in
        0)
            if (( terminal_flag_seen )); then
                COMPREPLY=()
                return
            fi
            local out
            out=$("${COMP_WORDS[0]}" "${config_args[@]}" --list-commands 2>/dev/null)
            COMPREPLY=( $(compgen -W "$out" -- "$cur") )
            ;;
        1)
            if (( terminal_flag_seen )); then
                COMPREPLY=()
                return
            fi
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
