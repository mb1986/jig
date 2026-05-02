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
            | Self::NodeHasProperties { .. } => ExitCode::JigFailure,
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
    fn all_variants_map_to_jig_failure() {
        let err = Error::ConfigNotFound {
            searched: String::new(),
            cwd: PathBuf::from("/"),
        };
        assert_eq!(err.exit_code(), ExitCode::JigFailure);
    }
}
