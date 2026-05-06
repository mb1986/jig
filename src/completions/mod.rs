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
    /// Zsh — primary target.
    Zsh,
    /// Bash — broad compatibility.
    Bash,
    /// Fish — popular among Rust users.
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
}
