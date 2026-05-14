//! Command-line argument parsing.
//!
//! `jig` follows the wrapper-tool invocation grammar (`SPEC.md`
//! §3.1):
//!
//! ```text
//! jig [JIG_FLAGS]... <command-or-alias> [profile] [PASSTHROUGH]...
//! ```
//!
//! `clap` parses only `JIG_FLAGS`. The positional region — command,
//! profile, and pass-through — is split out by [`parse_argv`]
//! before clap runs, then assigned to the `#[arg(skip)]` fields.
//! This is necessary because clap's `trailing_var_arg` consumes
//! the literal `--` separator, but `SPEC.md` §3.2 requires `--` to
//! be preserved verbatim in the pass-through region. clap also
//! does not propagate `allow_hyphen_values` through earlier
//! positionals, so `jig cmd -x ...` would otherwise fail.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Parser, ValueHint};

use crate::completions::Shell;

const USAGE_NOTES: &str = "\
Positional arguments:
  <COMMAND>      Command name or alias from jig.kdl.
  [PROFILE]      Optional profile name within COMMAND.
  [PASSTHROUGH]  Arguments appended verbatim to the resolved command line. May
                 include a literal `--` and tokens that look like flags.";

/// `jig` — run commands with arguments taken from a declarative
/// configuration file.
// Independent boolean toggles: `--list`, `--dry-run`, `--quiet`,
// and the two hidden completion-only flags don't share state, so
// the state-machine refactor clippy suggests would just bloat the
// type without expressing anything real.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Default, Parser)]
#[command(
    name = "jig",
    version,
    about,
    long_about = None,
    after_help = USAGE_NOTES,
)]
pub struct Cli {
    /// Use `<PATH>` instead of looking for `jig.kdl` / `.jig.kdl`
    /// in the current working directory.
    #[arg(long, value_name = "PATH", value_hint = ValueHint::FilePath)]
    pub config: Option<PathBuf>,

    /// List all configured commands, aliases, and profiles.
    #[arg(short = 'l', long)]
    pub list: bool,

    /// Print the resolved command (shell-quoted) and exit without
    /// executing.
    #[arg(short = 'n', long)]
    pub dry_run: bool,

    /// Suppress the pre-exec preview line that `jig` writes to
    /// stderr before spawning the resolved command. No effect on
    /// `--dry-run`, `--list`, or any other non-exec path.
    #[arg(short = 'q', long)]
    pub quiet: bool,

    /// Print a shell completion script for `<SHELL>` (zsh, bash,
    /// fish) to stdout. Typically piped into the shell's completion
    /// directory at install time. Does not require a config file.
    #[arg(long, value_name = "SHELL")]
    pub completions: Option<Shell>,

    /// Print every command name and alias from the loaded config,
    /// one per line. Hidden — used by completion scripts. Any
    /// failure to load or validate the config exits 0 with empty
    /// stdout so completion never breaks mid-tab.
    #[arg(
        long,
        hide = true,
        conflicts_with_all = ["list", "dry_run", "completions", "list_profiles"],
    )]
    pub list_commands: bool,

    /// Print every profile name attached to `<COMMAND>` (a command
    /// name or alias), one per line. Hidden — used by completion
    /// scripts. Unknown / ambiguous names produce empty output.
    #[arg(
        long,
        value_name = "COMMAND",
        hide = true,
        conflicts_with_all = ["list", "dry_run", "completions"],
    )]
    pub list_profiles: Option<String>,

    /// Filled by [`parse_argv`], not clap.
    #[arg(skip)]
    pub command: Option<String>,

    /// Filled by [`parse_argv`], not clap.
    #[arg(skip)]
    pub profile: Option<String>,

    /// Filled by [`parse_argv`], not clap.
    #[arg(skip)]
    pub passthrough: Vec<OsString>,
}

/// Parse `std::env::args_os()` into a fully-populated [`Cli`],
/// honoring `SPEC.md` §3.1 / §3.2.
///
/// Walks argv looking for jig's known flags; everything from the
/// first non-flag argument onward is positional / pass-through.
/// A literal `--` is consumed in two specific positions: as the
/// flags-vs-positional boundary before the command name (e.g.
/// `jig --config x.kdl -- cmd`), and in the profile slot (the
/// token immediately after `<command-or-alias>`) where it marks
/// "no profile selected". A `--` anywhere else in the positional
/// region is preserved verbatim.
#[must_use]
pub fn parse_argv() -> Cli {
    let argv: Vec<OsString> = std::env::args_os().collect();
    let (head, rest) = split_argv(&argv);
    let mut cli = Cli::parse_from(head);
    fill_positional(&mut cli, rest);
    cli
}

/// Split `argv` into `(head, rest)` where `head` is the binary name
/// plus jig's own flags (clap-parsed) and `rest` is everything
/// from the first non-flag positional onward.
///
/// Knows that `--config` takes a value. A literal `--` token
/// before the first positional is consumed as the flags/positional
/// boundary; one that appears AFTER a positional stays in `rest`.
fn split_argv(argv: &[OsString]) -> (&[OsString], &[OsString]) {
    let mut i = 1;
    while i < argv.len() {
        let arg = argv[i].to_string_lossy();
        if arg == "--" {
            // `--` between flags and positionals: consume it.
            return (&argv[..i], &argv[i + 1..]);
        }
        if !arg.starts_with('-') {
            // First positional. Stop here; everything from `i`
            // onward is command/profile/passthrough.
            return (&argv[..i], &argv[i..]);
        }
        // Hyphen-prefixed: a flag. `--config`, `--completions`,
        // and `--list-profiles` take values; everything else is
        // treated as standalone.
        if matches!(
            arg.as_ref(),
            "--config" | "--completions" | "--list-profiles"
        ) {
            // Skip `--<flag> <value>` (or just `--<flag>` if value
            // is missing — let clap error on that).
            i += 2;
        } else {
            i += 1;
        }
    }
    (argv, &[])
}

/// Emit the completion script for `shell` to stdout, per `SPEC.md`
/// §3.4. The script completes `jig`'s own flags and dispatches to
/// `jig --list-commands` / `jig --list-profiles` for dynamic
/// command, alias, and profile completion against the local
/// `jig.kdl`.
pub fn emit_completions(shell: Shell) {
    crate::completions::emit(shell);
}

fn fill_positional(cli: &mut Cli, rest: &[OsString]) {
    let mut iter = rest.iter();

    cli.command = iter.next().map(|s| s.to_string_lossy().into_owned());

    // Profile-slot interpretation per `SPEC.md` §3.1:
    //   - `--`     → consumed as the "no profile selected" marker.
    //   - bare     → the profile name (§2.9 forbids leading `-`).
    //   - `-…`     → no profile; first pass-through token (held).
    let mut held: Option<&OsString> = iter.next();
    if let Some(token) = held {
        let s = token.to_string_lossy();
        if s == "--" {
            held = None;
        } else if !s.starts_with('-') {
            cli.profile = Some(s.into_owned());
            held = iter.next();
        }
    }

    let mut passthrough: Vec<OsString> = Vec::new();
    if let Some(t) = held {
        passthrough.push(t.clone());
    }
    passthrough.extend(iter.cloned());
    cli.passthrough = passthrough;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(s: &str) -> OsString {
        OsString::from(s)
    }

    fn rest(args: &[&str]) -> Vec<OsString> {
        args.iter().map(|s| os(s)).collect()
    }

    #[test]
    fn no_positionals() {
        let mut cli = Cli::default();
        fill_positional(&mut cli, &[]);
        assert_eq!(cli.command, None);
        assert_eq!(cli.profile, None);
        assert!(cli.passthrough.is_empty());
    }

    #[test]
    fn command_only() {
        let mut cli = Cli::default();
        fill_positional(&mut cli, &rest(&["serve"]));
        assert_eq!(cli.command.as_deref(), Some("serve"));
        assert_eq!(cli.profile, None);
        assert!(cli.passthrough.is_empty());
    }

    #[test]
    fn command_and_profile() {
        let mut cli = Cli::default();
        fill_positional(&mut cli, &rest(&["serve", "qwen-coder"]));
        assert_eq!(cli.command.as_deref(), Some("serve"));
        assert_eq!(cli.profile.as_deref(), Some("qwen-coder"));
        assert!(cli.passthrough.is_empty());
    }

    #[test]
    fn command_profile_passthrough() {
        let mut cli = Cli::default();
        fill_positional(&mut cli, &rest(&["serve", "qwen-coder", "a", "b"]));
        assert_eq!(cli.command.as_deref(), Some("serve"));
        assert_eq!(cli.profile.as_deref(), Some("qwen-coder"));
        assert_eq!(cli.passthrough, rest(&["a", "b"]));
    }

    #[test]
    fn hyphen_token_after_command_is_passthrough_not_profile() {
        // §2.9: profile names cannot start with `-`.
        let mut cli = Cli::default();
        fill_positional(&mut cli, &rest(&["foo", "-x", "--abc", "-y"]));
        assert_eq!(cli.command.as_deref(), Some("foo"));
        assert_eq!(cli.profile, None);
        assert_eq!(cli.passthrough, rest(&["-x", "--abc", "-y"]));
    }

    #[test]
    fn double_dash_after_profile_is_preserved_in_passthrough() {
        // §3.2: `--` in pass-through is not stripped.
        let mut cli = Cli::default();
        fill_positional(&mut cli, &rest(&["serve", "qwen-coder", "--", "--abc"]));
        assert_eq!(cli.profile.as_deref(), Some("qwen-coder"));
        assert_eq!(cli.passthrough, rest(&["--", "--abc"]));
    }

    #[test]
    fn double_dash_in_profile_slot_is_consumed() {
        // §3.1: `--` immediately after the command is the
        // "no profile" marker and is consumed.
        let mut cli = Cli::default();
        fill_positional(&mut cli, &rest(&["foo", "--", "--abc"]));
        assert_eq!(cli.command.as_deref(), Some("foo"));
        assert_eq!(cli.profile, None);
        assert_eq!(cli.passthrough, rest(&["--abc"]));
    }

    #[test]
    fn double_dash_in_profile_slot_unblocks_bare_positional() {
        // The motivating case: a bare positional pass-through that
        // would otherwise be misread as a profile name.
        let mut cli = Cli::default();
        fill_positional(&mut cli, &rest(&["foo", "--", "bar"]));
        assert_eq!(cli.command.as_deref(), Some("foo"));
        assert_eq!(cli.profile, None);
        assert_eq!(cli.passthrough, rest(&["bar"]));
    }

    #[test]
    fn second_double_dash_after_profile_slot_marker_is_preserved() {
        // First `--` is consumed (no-profile marker); a second `--`
        // sits in the pass-through region and is preserved per §3.2.
        let mut cli = Cli::default();
        fill_positional(&mut cli, &rest(&["foo", "--", "--", "bar"]));
        assert_eq!(cli.command.as_deref(), Some("foo"));
        assert_eq!(cli.profile, None);
        assert_eq!(cli.passthrough, rest(&["--", "bar"]));
    }

    #[test]
    fn lone_double_dash_in_profile_slot_yields_empty_passthrough() {
        let mut cli = Cli::default();
        fill_positional(&mut cli, &rest(&["foo", "--"]));
        assert_eq!(cli.command.as_deref(), Some("foo"));
        assert_eq!(cli.profile, None);
        assert!(cli.passthrough.is_empty());
    }

    #[test]
    fn split_at_first_non_flag() {
        let argv = vec![
            os("jig"),
            os("--list"),
            os("--config"),
            os("x.kdl"),
            os("foo"),
            os("--bar"),
        ];
        let (head, rest) = split_argv(&argv);
        assert_eq!(
            head.iter()
                .map(|s| s.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["jig", "--list", "--config", "x.kdl"]
        );
        assert_eq!(
            rest.iter()
                .map(|s| s.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["foo", "--bar"]
        );
    }

    #[test]
    fn split_consumes_double_dash_before_command() {
        let argv = vec![os("jig"), os("--"), os("foo"), os("--bar")];
        let (head, rest) = split_argv(&argv);
        assert_eq!(head.len(), 1);
        assert_eq!(
            rest.iter()
                .map(|s| s.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["foo", "--bar"]
        );
    }
}
