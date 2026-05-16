//! Hand-rolled shell completion scripts.
//!
//! `jig --completions <SHELL>` prints the script for the requested
//! shell. The scripts know `jig`'s own flags statically and call
//! back into `jig --list-commands` / `jig --list-profiles` for
//! dynamic command, alias, and profile completion against the
//! local config.
//!
//! Supported shells (priority order): zsh, bash, fish. Other shells
//! are intentionally unsupported in v1.

use std::io::Write;

const ZSH: &str = include_str!("jig.zsh");
const BASH: &str = include_str!("jig.bash");
const FISH: &str = include_str!("jig.fish");

/// Shells for which `jig` can emit a completion script.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Shell {
    Zsh,
    Bash,
    Fish,
}

/// Write the completion script for `shell` to stdout.
///
/// Stdout writes are best-effort: a closed pipe at this stage just
/// means the consuming shell vanished, and there's no useful
/// diagnostic to surface.
pub fn emit(shell: Shell) {
    let script = match shell {
        Shell::Zsh => ZSH,
        Shell::Bash => BASH,
        Shell::Fish => FISH,
    };
    let _ = std::io::stdout().write_all(script.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zsh_starts_with_compdef() {
        assert!(ZSH.starts_with("#compdef jig"));
    }

    #[test]
    fn bash_registers_completion_function() {
        assert!(BASH.contains("complete -F _jig jig"));
    }

    #[test]
    fn fish_registers_completions_for_jig() {
        assert!(FISH.contains("complete -c jig"));
    }

    #[test]
    fn each_script_dispatches_to_list_commands() {
        for s in [ZSH, BASH, FISH] {
            assert!(
                s.contains("--list-commands"),
                "completion script must call jig --list-commands"
            );
            assert!(
                s.contains("--list-profiles"),
                "completion script must call jig --list-profiles"
            );
        }
    }

    #[test]
    fn each_script_forwards_config() {
        for s in [ZSH, BASH, FISH] {
            assert!(
                s.contains("--config"),
                "completion script must forward --config"
            );
        }
    }

    #[test]
    fn zsh_encodes_cat_conflicts() {
        // `_arguments` exclusion list for `--cat` should drop every
        // flag clap declares it conflicts with. Tokenize the
        // parenthesized exclusion list and check exact membership so
        // `-l` doesn't spuriously match the `-l` substring inside
        // `--list`.
        let line = ZSH
            .lines()
            .find(|l| l.contains("--cat[Dump"))
            .expect("zsh script must define a --cat spec");
        let parens = line
            .split_once('(')
            .and_then(|(_, rest)| rest.split_once(')'))
            .map(|(inner, _)| inner)
            .expect("zsh --cat spec must start with `(...)` exclusion list");
        let tokens: Vec<&str> = parens.split_whitespace().collect();
        for blocked in [
            "-l",
            "--list",
            "-n",
            "--dry-run",
            "-x",
            "--explain",
            "--completions",
        ] {
            assert!(
                tokens.contains(&blocked),
                "--cat exclusion list missing `{blocked}`: {tokens:?}"
            );
        }
    }

    #[test]
    fn zsh_suppresses_command_on_terminal_flag() {
        assert!(
            ZSH.contains("terminal_flag_seen"),
            "zsh script must track terminal flags to suppress command/profile candidates"
        );
    }

    #[test]
    fn bash_filters_conflicting_flags() {
        assert!(
            BASH.contains("_jig_filter_flags"),
            "bash script must define a flag-filtering helper"
        );
        assert!(
            BASH.contains("terminal_flag_seen"),
            "bash script must track terminal flags to suppress command/profile candidates"
        );
    }

    #[test]
    fn fish_uses_seen_argument_for_conflicts() {
        assert!(
            FISH.contains("__fish_seen_argument"),
            "fish script must use __fish_seen_argument to gate conflicting flags"
        );
        assert!(
            FISH.contains("__jig_terminal_flag_seen"),
            "fish script must gate command/profile dispatch on terminal flags"
        );
    }
}
