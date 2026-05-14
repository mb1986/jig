//! `--list` rendering per `SPEC.md` §3.4 / §7.1.
//!
//! Each command prints as a header line, optional `cwd:` / `env:` /
//! `defaults:` lines, and an optional `profiles:` sub-block. Each
//! profile in that sub-block has its own optional `cwd:` / `env:` /
//! `args:` sub-lines, showing the profile's own contributions only
//! (the view is static — `--dry-run` is the source of truth for what
//! a given invocation would actually execute).
//!
//! Default-side flag / positional / env-var values are rendered via
//! [`crate::format::format_args`] and [`crate::format::format_env_entries`],
//! so the `-` / `--` prefix synthesis (§2.5), `#true` / `#false`
//! handling (§2.4.1), and shell-quoting (§7.2) match what `--dry-run`
//! would produce. Profile-side values reuse the same renderers.
//!
//! When stdout is a terminal and `NO_COLOR` is unset, command names,
//! profile names, section labels, and parenthesized annotations are
//! styled with ANSI escapes. Otherwise the output is plain text and
//! byte-identical to the pre-color rendering shape (modulo the §7.1
//! example shape itself).

use std::io::IsTerminal;

use crate::config::{Argument, Command, CommandChild, Config, EnvEntry};
use crate::errors::Result;
use crate::format;

/// Render `config` to stdout per `SPEC.md` §7.1.
///
/// # Errors
///
/// Returns [`crate::errors::Error::ArgumentContainsNul`] if any
/// rendered value contains a NUL byte (rare, but [`format::format_args`]
/// can't quote it).
pub fn print(config: &Config) -> Result<()> {
    let theme = Theme::from_stdout();
    for (i, cmd) in config.commands.iter().enumerate() {
        if i > 0 {
            println!();
        }
        print_command(cmd, theme)?;
    }
    Ok(())
}

/// Width to which command-level section labels (`cwd:`, `env:`,
/// `defaults:`, `profiles:`) pad so the values that follow line up.
/// Picked once because the label set is small and stable.
const CMD_LABEL_WIDTH: usize = "defaults:".len();

/// Width for profile-level section labels (`cwd:`, `env:`, `args:`).
const PROFILE_LABEL_WIDTH: usize = "args:".len();

fn print_command(cmd: &Command, theme: Theme) -> Result<()> {
    match &cmd.alias {
        Some(alias) => println!(
            "{}  {}",
            theme.cmd_name(&cmd.name),
            theme.alias_annotation(&format!("(alias: {alias})"))
        ),
        None => println!("{}", theme.cmd_name(&cmd.name)),
    }

    if let Some((path, _)) = &cmd.cwd {
        println!(
            "  {}{}",
            kv_label(theme, "cwd:", CMD_LABEL_WIDTH),
            theme.value(path)
        );
    }

    if !cmd.env.is_empty() {
        let line = format::format_env_entries(&cmd.env)?;
        println!(
            "  {}{}",
            kv_label(theme, "env:", CMD_LABEL_WIDTH),
            theme.value(&line)
        );
    }

    let defaults: Vec<&Argument> = cmd
        .children
        .iter()
        .filter_map(|c| match c {
            CommandChild::Default(a) => Some(a),
            CommandChild::Profile { .. } => None,
        })
        .collect();
    if !defaults.is_empty() {
        let line = format::format_args(defaults)?;
        println!(
            "  {}{}",
            kv_label(theme, "defaults:", CMD_LABEL_WIDTH),
            theme.value(&line)
        );
    }

    let has_profiles = cmd
        .children
        .iter()
        .any(|c| matches!(c, CommandChild::Profile { .. }));
    if has_profiles {
        println!("  {}", theme.label("profiles:"));
        for child in &cmd.children {
            if let CommandChild::Profile {
                name,
                extends,
                cwd,
                args,
                env,
                ..
            } = child
            {
                print_profile(
                    theme,
                    name,
                    extends.as_ref().map(|(p, _)| p.as_str()),
                    cwd.as_ref().map(|(p, _)| p.as_str()),
                    args,
                    env,
                )?;
            }
        }
    }
    Ok(())
}

fn print_profile(
    theme: Theme,
    name: &str,
    extends: Option<&str>,
    cwd: Option<&str>,
    args: &[Argument],
    env: &[EnvEntry],
) -> Result<()> {
    match extends {
        Some(parent) => println!(
            "    {}  {}",
            theme.profile_name(name),
            theme.extends_annotation(&format!("(extends {parent})"))
        ),
        None => println!("    {}", theme.profile_name(name)),
    }
    if let Some(path) = cwd {
        println!(
            "      {}{}",
            kv_label(theme, "cwd:", PROFILE_LABEL_WIDTH),
            theme.value(path)
        );
    }
    if !env.is_empty() {
        let line = format::format_env_entries(env)?;
        println!(
            "      {}{}",
            kv_label(theme, "env:", PROFILE_LABEL_WIDTH),
            theme.value(&line)
        );
    }
    if !args.is_empty() {
        let line = format::format_args(args)?;
        println!(
            "      {}{}",
            kv_label(theme, "args:", PROFILE_LABEL_WIDTH),
            theme.value(&line)
        );
    }
    Ok(())
}

/// Render `label` (e.g. `"cwd:"`), styled, and pad it with spaces so
/// the value that follows starts at `width + 1` characters past the
/// label's start (the `+ 1` is the single mandatory separator space).
/// Padding is appended after the styled string in plain text so ANSI
/// escapes never count toward column alignment.
fn kv_label(theme: Theme, label: &str, width: usize) -> String {
    let padding = width.saturating_sub(label.len()) + 1;
    let mut out = theme.label(label);
    for _ in 0..padding {
        out.push(' ');
    }
    out
}

/// Terminal styling for `--list`. `Plain` returns its input unchanged;
/// `Ansi` wraps names, labels, and annotations in ANSI escape codes.
///
/// A small fixed role set is styled (command name, profile name,
/// section label, and the two annotation kinds) — coloring is a
/// readability cue, not a syntax highlighter; flag keys, values, and
/// env-var names are intentionally left to the default foreground so
/// the existing `format::format_args` / `format::format_env_entries`
/// renderers can be reused unchanged.
#[derive(Debug, Clone, Copy)]
enum Theme {
    Plain,
    Ansi,
}

impl Theme {
    /// Pick a theme based on stdout: ANSI iff stdout is a terminal
    /// and `NO_COLOR` is unset (the de-facto convention from
    /// <https://no-color.org>). Otherwise plain.
    fn from_stdout() -> Self {
        if std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() {
            Self::Ansi
        } else {
            Self::Plain
        }
    }

    /// Bold blue — the command name on the header line.
    fn cmd_name(self, s: &str) -> String {
        match self {
            Self::Plain => s.to_string(),
            Self::Ansi => format!("\x1b[1;34m{s}\x1b[0m"),
        }
    }

    /// Italic magenta — profile names inside the `profiles:` sub-block.
    /// Distinct hue and weight from the dim section labels so the
    /// two roles don't visually blend on a dense listing.
    fn profile_name(self, s: &str) -> String {
        match self {
            Self::Plain => s.to_string(),
            Self::Ansi => format!("\x1b[3;35m{s}\x1b[0m"),
        }
    }

    /// Dim (faint) — section labels (`cwd:`, `env:`, `defaults:`,
    /// `args:`, `profiles:`). Lowered contrast rather than added
    /// weight, so the values that follow stay the foreground content
    /// and the labels read as scaffolding.
    fn label(self, s: &str) -> String {
        match self {
            Self::Plain => s.to_string(),
            Self::Ansi => format!("\x1b[2m{s}\x1b[0m"),
        }
    }

    /// Green — `(alias: ...)` annotations on the command header line.
    fn alias_annotation(self, s: &str) -> String {
        match self {
            Self::Plain => s.to_string(),
            Self::Ansi => format!("\x1b[32m{s}\x1b[0m"),
        }
    }

    /// Amber (256-color goldenrod) — `(extends ...)` annotations on
    /// inheriting profile header lines.
    fn extends_annotation(self, s: &str) -> String {
        match self {
            Self::Plain => s.to_string(),
            Self::Ansi => format!("\x1b[38;5;214m{s}\x1b[0m"),
        }
    }

    /// Bright white — the value rendered after a section label
    /// (cwd paths, env lines, defaults, profile args). Pairs with
    /// the dim labels above so the data is the foreground content
    /// and the labels read as scaffolding.
    fn value(self, s: &str) -> String {
        match self {
            Self::Plain => s.to_string(),
            Self::Ansi => format!("\x1b[97m{s}\x1b[0m"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_plain_returns_input_unchanged() {
        let t = Theme::Plain;
        assert_eq!(t.cmd_name("llama-server"), "llama-server");
        assert_eq!(t.profile_name("qwen-coder"), "qwen-coder");
        assert_eq!(t.label("defaults:"), "defaults:");
        assert_eq!(t.alias_annotation("(alias: serve)"), "(alias: serve)");
        assert_eq!(t.extends_annotation("(extends qwen)"), "(extends qwen)");
        assert_eq!(t.value("--host 0.0.0.0"), "--host 0.0.0.0");
    }

    #[test]
    fn theme_ansi_wraps_cmd_name_in_bold_blue() {
        assert_eq!(
            Theme::Ansi.cmd_name("llama-server"),
            "\x1b[1;34mllama-server\x1b[0m"
        );
    }

    #[test]
    fn theme_ansi_wraps_profile_name_in_italic_magenta() {
        assert_eq!(
            Theme::Ansi.profile_name("qwen-coder"),
            "\x1b[3;35mqwen-coder\x1b[0m"
        );
    }

    #[test]
    fn theme_ansi_wraps_label_in_dim() {
        assert_eq!(Theme::Ansi.label("args:"), "\x1b[2margs:\x1b[0m");
    }

    #[test]
    fn theme_ansi_wraps_alias_annotation_in_green() {
        assert_eq!(
            Theme::Ansi.alias_annotation("(alias: serve)"),
            "\x1b[32m(alias: serve)\x1b[0m"
        );
    }

    #[test]
    fn theme_ansi_wraps_extends_annotation_in_amber() {
        // Amber here is 256-color goldenrod (38;5;214), distinct from
        // the alias-annotation green so the two annotation kinds read
        // apart on dense listings.
        assert_eq!(
            Theme::Ansi.extends_annotation("(extends qwen-coder)"),
            "\x1b[38;5;214m(extends qwen-coder)\x1b[0m"
        );
    }

    #[test]
    fn theme_ansi_wraps_value_in_bright_white() {
        assert_eq!(
            Theme::Ansi.value("--host 0.0.0.0"),
            "\x1b[97m--host 0.0.0.0\x1b[0m"
        );
    }

    #[test]
    fn kv_label_pads_short_label_to_width_plus_separator() {
        // "cwd:" is 4 chars; width 9 → 5 padding + 1 separator = 6 trailing spaces.
        assert_eq!(kv_label(Theme::Plain, "cwd:", 9), "cwd:      ");
    }

    #[test]
    fn kv_label_exact_width_emits_one_separator() {
        // "defaults:" is exactly 9 chars; width 9 → 0 padding + 1 separator.
        assert_eq!(kv_label(Theme::Plain, "defaults:", 9), "defaults: ");
    }

    #[test]
    fn kv_label_under_ansi_pads_with_plain_spaces() {
        // ANSI escapes around the label must not count toward alignment;
        // padding is appended as plain spaces after the styled label.
        let s = kv_label(Theme::Ansi, "cwd:", 9);
        assert_eq!(s, "\x1b[2mcwd:\x1b[0m      ");
    }
}
