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

use crate::config::{Argument, Command, CommandChild, Config, EnvEntry};
use crate::errors::Result;
use crate::format;
use crate::theme::Theme;

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

#[cfg(test)]
mod tests {
    use super::*;

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
