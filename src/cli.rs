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

use clap::{CommandFactory, Parser, ValueHint};
use clap_complete::Shell;

const USAGE_NOTES: &str = "\
Positional arguments:
  <COMMAND>      Command name or alias from jig.kdl.
  [PROFILE]      Optional profile name within COMMAND.
  [PASSTHROUGH]  Arguments appended verbatim to the resolved command line. May
                 include a literal `--` and tokens that look like flags.";

/// `jig` — run commands with arguments taken from a declarative
/// configuration file.
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

    /// Generate a shell completion script for `<SHELL>`. Hidden
    /// from `--help` because it is rarely run directly by humans.
    /// Does not require a config file.
    #[arg(long, value_name = "SHELL", hide = true)]
    pub completions: Option<Shell>,

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
/// `--` is preserved when it appears in the positional region;
/// it is consumed only when used as a separator before the
/// command (e.g. `jig --config x.kdl -- cmd`).
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
        // Hyphen-prefixed: a flag. `--config` and `--completions`
        // take values; everything else is treated as standalone.
        if matches!(arg.as_ref(), "--config" | "--completions") {
            // Skip `--<flag> <value>` (or just `--<flag>` if value
            // is missing — let clap error on that).
            i += 2;
        } else {
            i += 1;
        }
    }
    (argv, &[])
}

/// Emit the static completion script for `shell` to stdout, per
/// `SPEC.md` §3.4. The completion completes `jig`'s own flags and
/// tells the shell that the next positional is "a command name";
/// dynamic completion of command/profile names from `jig.kdl` is
/// deferred (see `FUTURE.md`).
pub fn emit_completions(shell: Shell) {
    let mut cmd = Cli::command();
    clap_complete::generate(shell, &mut cmd, "jig", &mut std::io::stdout());
}

fn fill_positional(cli: &mut Cli, rest: &[OsString]) {
    let mut iter = rest.iter();

    cli.command = iter.next().map(|s| s.to_string_lossy().into_owned());

    // Profile is the next token unless it starts with `-` (per
    // `SPEC.md` §2.9 profile names cannot start with `-`, so a
    // hyphen-prefixed token at this position must be pass-through).
    let mut held: Option<&OsString> = iter.next();
    if let Some(token) = held
        && !token.to_string_lossy().starts_with('-')
    {
        cli.profile = Some(token.to_string_lossy().into_owned());
        held = iter.next();
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
    fn double_dash_after_command_with_no_profile_is_preserved() {
        let mut cli = Cli::default();
        fill_positional(&mut cli, &rest(&["foo", "--", "--abc"]));
        assert_eq!(cli.command.as_deref(), Some("foo"));
        assert_eq!(cli.profile, None);
        assert_eq!(cli.passthrough, rest(&["--", "--abc"]));
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
