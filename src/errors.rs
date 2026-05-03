//! Crate-level typed error, `Result` alias, and exit-code mapping.
//!
//! Errors render through `miette` for diagnostic-quality output per
//! `SPEC.md` §7.4. Exit codes follow `SPEC.md` §3.5: 125 for any
//! `jig`-internal failure, 126 / 127 for exec problems (added when
//! `exec.rs` lands), anything else is propagated from the executed
//! child.

use std::path::PathBuf;

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

/// Crate-wide `Result` alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Wrapper-tool exit codes per `SPEC.md` §3.5.
///
/// Successful runs and propagated child exit codes are plain `i32`
/// values returned out of the success path; this enum names only the
/// codes that `jig` itself originates. Additional variants
/// (`NotExecutable`, `NotFound`) land alongside `exec.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    /// `jig` itself failed: missing config, parse error, constraint
    /// violation, unknown command/profile/alias, bad CLI usage.
    JigFailure,
}

impl ExitCode {
    /// Numeric value, suitable for `std::process::exit`.
    #[must_use]
    pub const fn as_i32(self) -> i32 {
        match self {
            Self::JigFailure => 125,
        }
    }
}

/// Anything that can go wrong inside `jig` itself, prior to handing
/// off control to the resolved command. Each variant renders as a
/// `miette` diagnostic and maps to an [`ExitCode`] via
/// [`Error::exit_code`].
#[derive(Debug, Error, Diagnostic)]
pub enum Error {
    /// No `jig.kdl` (or `.jig.kdl`) was found in the search location
    /// and no `--config` path was supplied. Per `SPEC.md` §2.1.
    #[error("config file not found\n  searched: {searched}\n  in directory: {}", cwd.display())]
    #[diagnostic(help("create a jig.kdl file with at least one command definition"))]
    ConfigNotFound {
        /// Comma-separated rendering of the paths that were probed.
        searched: String,
        /// The directory in which the search ran.
        cwd: PathBuf,
    },

    /// Reading the config file failed at the OS level (permission
    /// denied, not a regular file, etc.).
    #[error("could not read config file: {}", path.display())]
    ConfigIo {
        /// The path that failed to read.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The config file is not valid KDL. The wrapped diagnostic
    /// carries the source span and label produced by the `kdl`
    /// parser; we surface it as-is per `SPEC.md` §7.4.
    #[error(transparent)]
    #[diagnostic(transparent)]
    KdlParse(#[from] kdl::KdlError),

    /// A flag node (KDL node with at least one value) carries more
    /// than one value. `jig`'s argument model treats a node as
    /// either a flag (one value) or a positional (no values); see
    /// `SPEC.md` §2.4.
    #[error("flag has multiple values; expected exactly one")]
    #[diagnostic(help(
        "in jig.kdl, a flag is a KDL node with exactly one value (e.g. `host \"0.0.0.0\"`)"
    ))]
    FlagMultipleValues {
        /// Source the span points into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the offending node.
        #[label("flag with multiple values")]
        span: SourceSpan,
    },

    /// A KDL property (`key=value` on a node) was used somewhere in
    /// the config. `jig`'s model does not use properties; nested
    /// data is expressed via child nodes. See Q5 / `SPEC.md` §2.4.
    #[error("KDL properties (key=value) are not supported; use child nodes instead")]
    NodeHasProperties {
        /// Source the span points into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the offending property.
        #[label("property here")]
        span: SourceSpan,
    },

    /// A position that requires a string value (currently only the
    /// command alias) carried a non-string KDL value (number, bool,
    /// null).
    #[error("expected a string here")]
    #[diagnostic(help(
        "an alias is the first KDL value on a command node, written as a quoted string"
    ))]
    ExpectedString {
        /// Source the span points into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the offending entry.
        #[label("not a string")]
        span: SourceSpan,
    },

    /// A command, alias, or profile name starts with `-`. Per
    /// `SPEC.md` §2.9.
    #[error("{kind} name {name:?} must not start with `-`")]
    #[diagnostic(help(
        "names starting with `-` would be ambiguous with `jig`'s own flags; rename it"
    ))]
    LeadingDashName {
        /// Which kind of name this was: `"command"`, `"alias"`, or
        /// `"profile"`.
        kind: &'static str,
        /// The offending name.
        name: String,
        /// Source the span points into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the offending name.
        #[label("starts with `-`")]
        span: SourceSpan,
    },

    /// Two top-level commands share the same name. Per `SPEC.md`
    /// §2.9.
    #[error("command {name:?} is defined more than once")]
    #[diagnostic(help("each command name may appear at most once"))]
    DuplicateCommand {
        /// The offending command name.
        name: String,
        /// Source the spans point into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the first occurrence.
        #[label("first defined here")]
        first: SourceSpan,
        /// Span of the second occurrence.
        #[label("also defined here")]
        second: SourceSpan,
    },

    /// Two commands share the same alias. Per `SPEC.md` §2.9.
    #[error("alias {alias:?} is defined more than once")]
    #[diagnostic(help("each alias may be used by at most one command"))]
    DuplicateAlias {
        /// The offending alias.
        alias: String,
        /// Source the spans point into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the first alias site.
        #[label("first defined here")]
        first: SourceSpan,
        /// Span of the second alias site.
        #[label("also defined here")]
        second: SourceSpan,
    },

    /// An alias on one command collides with a command name on a
    /// different command. Per `SPEC.md` §2.9. (Same-command
    /// self-aliasing — `foo "foo" {...}` — is allowed.)
    #[error("alias {name:?} collides with a command of the same name")]
    #[diagnostic(help("rename the alias or the command"))]
    CommandAliasCollision {
        /// The colliding name.
        name: String,
        /// Source the spans point into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the alias site.
        #[label("alias here")]
        alias_span: SourceSpan,
        /// Span of the command-name site.
        #[label("command name here")]
        command_span: SourceSpan,
    },

    /// Two profiles within the same command share a name. Per
    /// `SPEC.md` §2.9.
    #[error("profile {name:?} is defined more than once in command {command:?}")]
    #[diagnostic(help("profile names must be unique within a command"))]
    DuplicateProfile {
        /// The profile name.
        name: String,
        /// The owning command.
        command: String,
        /// Source the spans point into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the first profile site.
        #[label("first defined here")]
        first: SourceSpan,
        /// Span of the second profile site.
        #[label("also defined here")]
        second: SourceSpan,
    },

    /// Two flags in the same scope (a command's defaults or a
    /// single profile body) resolve to the same CLI form. Per
    /// `SPEC.md` §2.9. The comparison is on the *resolved* key
    /// (after §2.5 prefix synthesis), so `host` and `--host` are
    /// duplicates.
    #[error("flag {flag} is set more than once in this scope")]
    #[diagnostic(help(
        "each flag key may appear at most once within a scope; `host \"a\"` and `--host \"b\"` both resolve to `--host` and so are duplicates"
    ))]
    DuplicateFlagKey {
        /// The resolved CLI key (e.g. `--host`).
        flag: String,
        /// Source the spans point into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the first flag site.
        #[label("first set here")]
        first: SourceSpan,
        /// Span of the second flag site.
        #[label("also set here")]
        second: SourceSpan,
    },
}

impl Error {
    /// Wrapper-tool exit code for this error per `SPEC.md` §3.5.
    /// Every variant currently maps to [`ExitCode::JigFailure`];
    /// exec-related variants will be added in later steps.
    #[must_use]
    pub const fn exit_code(&self) -> ExitCode {
        match self {
            Self::ConfigNotFound { .. }
            | Self::ConfigIo { .. }
            | Self::KdlParse(_)
            | Self::FlagMultipleValues { .. }
            | Self::NodeHasProperties { .. }
            | Self::ExpectedString { .. }
            | Self::LeadingDashName { .. }
            | Self::DuplicateCommand { .. }
            | Self::DuplicateAlias { .. }
            | Self::CommandAliasCollision { .. }
            | Self::DuplicateProfile { .. }
            | Self::DuplicateFlagKey { .. } => ExitCode::JigFailure,
        }
    }
}

/// Render an error through `miette`'s graphical handler with the
/// `unicode_nocolor` theme. Used by snapshot tests; production
/// rendering uses `miette`'s default handler via `Report`'s `Debug`
/// impl so users see colored output when stderr is a TTY.
#[cfg(test)]
fn render_for_snapshot<D: Diagnostic>(err: &D) -> String {
    let mut out = String::new();
    miette::GraphicalReportHandler::new()
        .with_theme(miette::GraphicalTheme::unicode_nocolor())
        .render_report(&mut out, err)
        .expect("invariant: writing into a String never fails");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jig_failure_is_125() {
        assert_eq!(ExitCode::JigFailure.as_i32(), 125);
    }

    #[test]
    fn config_not_found_renders() {
        let err = Error::ConfigNotFound {
            searched: "./jig.kdl, ./.jig.kdl".to_string(),
            cwd: PathBuf::from("/home/user/project"),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn flag_multiple_values_renders() {
        let src = NamedSource::new("jig.kdl", "host \"0.0.0.0\" \"extra\"\n".to_string());
        let err = Error::FlagMultipleValues {
            src,
            span: SourceSpan::from((0, 22)),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn node_has_properties_renders() {
        let src = NamedSource::new("jig.kdl", "host port=8090\n".to_string());
        let err = Error::NodeHasProperties {
            src,
            span: SourceSpan::from((5, 9)),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn kdl_parse_renders() {
        let kdl_err = "foo 1.\n".parse::<kdl::KdlDocument>().unwrap_err();
        let err = Error::KdlParse(kdl_err);
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn expected_string_renders() {
        let src = NamedSource::new("jig.kdl", "foo 42 {}\n".to_string());
        let err = Error::ExpectedString {
            src,
            span: SourceSpan::from((4, 2)),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn leading_dash_name_renders() {
        let src = NamedSource::new("jig.kdl", "-bad-cmd {}\n".to_string());
        let err = Error::LeadingDashName {
            kind: "command",
            name: "-bad-cmd".to_string(),
            src,
            span: SourceSpan::from((0, 8)),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn duplicate_command_renders() {
        let src = NamedSource::new("jig.kdl", "llama-server {}\nllama-server {}\n".to_string());
        let err = Error::DuplicateCommand {
            name: "llama-server".to_string(),
            src,
            first: SourceSpan::from((0, 12)),
            second: SourceSpan::from((16, 12)),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn duplicate_alias_renders() {
        let src = NamedSource::new(
            "jig.kdl",
            "llama-server \"serve\" {}\ngemma-server \"serve\" {}\n".to_string(),
        );
        let err = Error::DuplicateAlias {
            alias: "serve".to_string(),
            src,
            first: SourceSpan::from((13, 7)),
            second: SourceSpan::from((37, 7)),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn command_alias_collision_renders() {
        let src = NamedSource::new(
            "jig.kdl",
            "serve {}\nllama-server \"serve\" {}\n".to_string(),
        );
        let err = Error::CommandAliasCollision {
            name: "serve".to_string(),
            src,
            alias_span: SourceSpan::from((22, 7)),
            command_span: SourceSpan::from((0, 5)),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn duplicate_profile_renders() {
        let src = NamedSource::new("jig.kdl", "foo {\n  fast {}\n  fast {}\n}\n".to_string());
        let err = Error::DuplicateProfile {
            name: "fast".to_string(),
            command: "foo".to_string(),
            src,
            first: SourceSpan::from((8, 4)),
            second: SourceSpan::from((18, 4)),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn duplicate_flag_key_renders() {
        let src = NamedSource::new(
            "jig.kdl",
            "foo {\n  host \"a\"\n  --host \"b\"\n}\n".to_string(),
        );
        let err = Error::DuplicateFlagKey {
            flag: "--host".to_string(),
            src,
            first: SourceSpan::from((8, 4)),
            second: SourceSpan::from((19, 6)),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn all_variants_map_to_jig_failure() {
        let err = Error::ConfigNotFound {
            searched: String::new(),
            cwd: PathBuf::from("/"),
        };
        assert_eq!(err.exit_code(), ExitCode::JigFailure);
    }
}
