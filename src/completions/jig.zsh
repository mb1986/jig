#compdef jig

# Expand a leading `~/` or bare `~` in $1 to $HOME. A captured token
# (e.g. the value typed after `--config`) keeps `~` literal — shells
# only expand `~` at parse time on unquoted words, never on values
# pulled from a variable. Without this, `jig --config ~/x.kdl
# --list-commands` fails to find the file and completion silently
# returns no candidates. `~user/...` is intentionally left alone:
# zsh's built-in `${~var}` would handle it, but on an unknown user
# it also writes "no such user" to stderr — noisy during tab.
_jig_expand_tilde() {
    case "$1" in
        "~")   print -rn -- "$HOME" ;;
        "~/"*) print -rn -- "$HOME/${1#"~/"}" ;;
        *)     print -rn -- "$1" ;;
    esac
}

_jig() {
    typeset -A opt_args
    local context curcontext="$curcontext" state state_descr line
    local ret=1
    local -a config_args positionals

    # Mirror jig's argv split: parse jig flags only before the first
    # positional command. After that, every token is command/profile/
    # pass-through context, even if it starts with `-`.
    local i word
    local skip_next=0
    local scanning_flags=1
    for (( i = 2; i < CURRENT; i++ )); do
        word="${words[$i]}"
        if (( scanning_flags )); then
            if (( skip_next )); then
                skip_next=0
                continue
            fi
            if [[ "$word" == "--" ]]; then
                scanning_flags=0
                continue
            fi
            if [[ "$word" == "--config" ]]; then
                if (( i + 1 < CURRENT )); then
                    config_args=("--config" "$(_jig_expand_tilde "${words[$((i + 1))]}")")
                fi
                skip_next=1
                continue
            fi
            if [[ "$word" == --config=* ]]; then
                config_args=("--config=$(_jig_expand_tilde "${word#--config=}")")
                continue
            fi
            if [[ "$word" == "--list-profiles" || "$word" == "--completions" ]]; then
                skip_next=1
                continue
            fi
            if [[ "$word" == -* ]]; then
                continue
            fi
            scanning_flags=0
        fi
        positionals+=("$word")
    done

    if (( ${#positionals[@]} > 0 )) && [[ "${words[CURRENT]}" == -* ]]; then
        _files
        return
    fi

    _arguments -s -S -A "-*" -C \
        '(-h --help)-h[Print help]' \
        '(-h --help)--help[Print help]' \
        '(-V --version)-V[Print version]' \
        '(-V --version)--version[Print version]' \
        '(-l --list)-l[List configured commands and profiles]' \
        '(-l --list)--list[List configured commands and profiles]' \
        '(-n --dry-run)-n[Print the resolved command without running it]' \
        '(-n --dry-run)--dry-run[Print the resolved command without running it]' \
        '(-q --quiet)-q[Suppress the pre-exec preview line]' \
        '(-q --quiet)--quiet[Suppress the pre-exec preview line]' \
        '--config=[Use this config file]:path:_files' \
        '--completions=[Print a shell completion script]:shell:(zsh bash fish)' \
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
            local cmd="${positionals[1]}"
            local -a candidates
            if [[ -n "$cmd" ]]; then
                candidates=("${(@f)$("${words[1]}" "${config_args[@]}" --list-profiles "$cmd" 2>/dev/null)}")
                _describe -t profiles 'profile' candidates && ret=0
            fi
            ;;
    esac

    return ret
}

_jig "$@"
