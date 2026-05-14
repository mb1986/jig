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
/// values returned out of the success path; this enum names only
/// the codes that `jig` itself originates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    /// `jig` itself failed: missing config, parse error, constraint
    /// violation, unknown command/profile/alias, bad CLI usage.
    JigFailure,
    /// The resolved command was found but is not executable
    /// (permission denied, not a regular file, ...).
    NotExecutable,
    /// The resolved command was not found in `$PATH` (or as a
    /// path).
    NotFound,
}

impl ExitCode {
    /// Numeric value, suitable for `std::process::exit`.
    #[must_use]
    pub const fn as_i32(self) -> i32 {
        match self {
            Self::JigFailure => 125,
            Self::NotExecutable => 126,
            Self::NotFound => 127,
        }
    }
}

/// Anything that can go wrong inside `jig` itself, prior to handing
/// off control to the resolved command. Each variant renders as a
/// `miette` diagnostic and maps to an [`ExitCode`] via
/// [`Error::exit_code`].
#[derive(Debug, Error, Diagnostic)]
pub enum Error {
    /// No `jig.kdl` (or `.jig.kdl`) was found anywhere in the upward
    /// search range and no `--config` path was supplied. Per
    /// `SPEC.md` §2.1.
    #[error("config file not found\n  searched: {searched}\n  from: {} up to: {}", from.display(), up_to.display())]
    #[diagnostic(help("create a jig.kdl file with at least one command definition"))]
    ConfigNotFound {
        /// Comma-separated list of file names that were probed in
        /// each ancestor directory (e.g. `"jig.kdl, .jig.kdl"`).
        searched: String,
        /// The starting directory of the upward walk (CWD).
        from: PathBuf,
        /// The last directory actually checked — either `$HOME` (if
        /// it appeared in the ancestor chain) or the filesystem
        /// root.
        up_to: PathBuf,
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

    /// A command, alias, or profile name starts with `+`. Per
    /// `SPEC.md` §2.5: a leading `+` on a flag key is the explicit
    /// append marker, and so is reserved on names of all kinds.
    #[error("{kind} name {name:?} must not start with `+`")]
    #[diagnostic(help(
        "a leading `+` is reserved for the explicit append marker on flag keys (`SPEC.md` §2.5); rename it"
    ))]
    LeadingPlusName {
        /// Which kind of name this was: `"command"`, `"alias"`, or
        /// `"profile"`.
        kind: &'static str,
        /// The offending name.
        name: String,
        /// Source the span points into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the offending name.
        #[label("starts with `+`")]
        span: SourceSpan,
    },

    /// A flag node was written as `+key #null`. The `+` append marker
    /// asks for a separate own-position emission, but `#null` is a
    /// position-only placeholder that never emits — combining them is
    /// meaningless. Per `SPEC.md` §2.4.3 / §2.5.
    #[error("the `+` append marker is not allowed on a `#null` placeholder")]
    #[diagnostic(help(
        "`#null` declares a position without emitting; the `+` marker has nothing to emit separately. Drop the `+`, or use a real value (or `#false`) if you wanted suppression"
    ))]
    NullWithAppendMarker {
        /// Source the span points into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the offending node name.
        #[label("`+` on `#null` placeholder")]
        span: SourceSpan,
    },

    /// A flag node was written as `+key value` with `key` empty
    /// after the marker is stripped. The `+` alone is not a valid
    /// flag key.
    #[error("flag key is empty after the `+` append marker")]
    #[diagnostic(help(
        "write the marker followed by the actual flag key (e.g. `+I \"/path\"`, `+host \"x\"`)"
    ))]
    EmptyKeyAfterMarker {
        /// Source the span points into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the offending node name.
        #[label("`+` with no key")]
        span: SourceSpan,
    },

    /// A KDL node carries a type annotation (`(name)node ...`) that
    /// is not recognized in this position. The only annotation
    /// defined in v1 is `(env)` on argument-shaped nodes inside a
    /// command or profile body.
    #[error("unknown type annotation `({annotation})`")]
    #[diagnostic(help(
        "the only annotation supported in v1 is `(env)`, on a node inside a command or profile body declaring an environment variable"
    ))]
    UnknownTypeAnnotation {
        /// The annotation text as written (without the surrounding
        /// parentheses).
        annotation: String,
        /// Source the span points into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the annotation.
        #[label("not a recognized annotation here")]
        span: SourceSpan,
    },

    /// An `(env)`-annotated node has a child block (`{ ... }`).
    /// Env-var declarations carry exactly one value; they do not
    /// have profile-like bodies.
    #[error("`(env)` declaration must not have a child block")]
    #[diagnostic(help(
        "an env-var declaration is `(env)NAME \"value\"` or `(env)NAME #false`; remove the `{{ ... }}` block"
    ))]
    EnvOnNodeWithChildren {
        /// Source the span points into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the offending node name.
        #[label("env declaration with body")]
        span: SourceSpan,
    },

    /// An `(env)`-annotated node has no value. Env vars require
    /// either a string/number value or the literal `#false` (unset).
    #[error("`(env)` declaration requires a value")]
    #[diagnostic(help(
        "write `(env)NAME \"value\"` to set the variable or `(env)NAME #false` to unset it"
    ))]
    EnvNoValue {
        /// Source the span points into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the offending node name.
        #[label("env declaration with no value")]
        span: SourceSpan,
    },

    /// An `(env)`-annotated node carries more than one value. An env
    /// declaration takes exactly one value (string / number) or
    /// `#false` to unset.
    #[error("`(env)` declaration has multiple values; expected exactly one")]
    #[diagnostic(help(
        "write `(env)NAME \"value\"` with a single value, or `(env)NAME #false` to unset"
    ))]
    EnvMultipleValues {
        /// Source the span points into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the offending extra value.
        #[label("extra value here")]
        span: SourceSpan,
    },

    /// An `(env)`-annotated node has a value of a kind that does
    /// not make sense for an env var (`#true`, `#null`).
    #[error("`(env)` declaration has an invalid value")]
    #[diagnostic(help(
        "use a string/number for a literal value (e.g. `(env)PORT \"8090\"`) or `#false` to unset; `#true` and `#null` are not valid"
    ))]
    EnvInvalidValue {
        /// Source the span points into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the offending value.
        #[label("not a valid env value")]
        span: SourceSpan,
    },

    /// An `(env)`-annotated node was written with the `+` explicit
    /// append marker. Env vars do not repeat (POSIX assigns one
    /// value per name) so the marker is meaningless on them.
    #[error("the `+` append marker is not allowed on `(env)` declarations")]
    #[diagnostic(help(
        "remove the leading `+` — env vars take a single value per name; declare the variable once per scope"
    ))]
    EnvWithAppendMarker {
        /// Source the span points into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the offending node name.
        #[label("`+` on env declaration")]
        span: SourceSpan,
    },

    /// An `(env)`-declared variable has a name that does not match
    /// the POSIX-portable env-var pattern `[A-Za-z_][A-Za-z0-9_]*`.
    #[error("env-var name {name:?} is not a valid POSIX-portable identifier")]
    #[diagnostic(help(
        "env-var names must match `[A-Za-z_][A-Za-z0-9_]*` (start with a letter or underscore, then letters / digits / underscores)"
    ))]
    EnvNameInvalid {
        /// The offending name.
        name: String,
        /// Source the span points into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the offending name.
        #[label("invalid env-var name")]
        span: SourceSpan,
    },

    /// Two `(env)` declarations share a name within a single scope
    /// (one command's defaults, or one profile body). Per
    /// `SPEC.md` §2.9.
    #[error("env-var {name:?} is declared more than once in {scope}")]
    #[diagnostic(help(
        "each env-var name may appear at most once per scope; combine the declarations or move one to a profile to override"
    ))]
    DuplicateEnvName {
        /// The offending env-var name.
        name: String,
        /// A short description of the scope (e.g. "command \"foo\"
        /// defaults" or "profile \"fast\" of command \"foo\"") for
        /// the diagnostic message.
        scope: String,
        /// Source the spans point into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the first declaration.
        #[label("first declared here")]
        first: SourceSpan,
        /// Span of the second declaration.
        #[label("also declared here")]
        second: SourceSpan,
    },

    /// Two top-level commands share the same name but at least one
    /// occurrence is missing an alias. Per `SPEC.md` §2.9 a
    /// duplicated command name is permitted only when every
    /// occurrence has a distinct alias — otherwise the unaliased
    /// entry is unreachable.
    #[error("command {name:?} is defined more than once but at least one occurrence has no alias")]
    #[diagnostic(help(
        "when a command name appears more than once, every occurrence must declare an alias so each entry can still be invoked"
    ))]
    DuplicateCommandWithoutAlias {
        /// The offending command name.
        name: String,
        /// Source the spans point into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the chronologically earlier occurrence.
        #[label("defined here")]
        first: SourceSpan,
        /// Span of the chronologically later occurrence.
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

    /// A `cwd="<path>"` property carries a non-string value or an
    /// empty string. Per `SPEC.md` §2.12 the value must be a
    /// non-empty path string.
    #[error("`cwd` value must be a non-empty path string")]
    #[diagnostic(help(
        "write `cwd=\"/abs/path\"` or `cwd=\"rel/path\"` (relative paths resolve against the config-file directory)"
    ))]
    CwdBadValue {
        /// Source the span points into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the offending value entry.
        #[label("not a valid cwd value")]
        span: SourceSpan,
    },

    /// A node carried more than one `cwd=` property. Per `SPEC.md`
    /// §2.9 / §2.12 each command and profile node may have at most
    /// one.
    #[error("`cwd` is specified more than once on this node")]
    #[diagnostic(help(
        "remove one of the `cwd=` properties; a node may declare the working directory at most once"
    ))]
    DuplicateCwd {
        /// Source the spans point into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the first `cwd=` site.
        #[label("first `cwd` here")]
        first: SourceSpan,
        /// Span of the second `cwd=` site.
        #[label("also here")]
        second: SourceSpan,
    },

    /// A resolved cwd path is not valid UTF-8 and so cannot be shell-
    /// quoted for `--dry-run` (or the pre-exec preview). This happens
    /// when the config-file directory itself is non-UTF-8 (e.g. the
    /// user invoked `jig` from a directory whose name is not valid
    /// UTF-8). Execution itself does not require UTF-8 — only the
    /// shell-quoted preview / dry-run rendering does.
    #[error("resolved cwd path is not valid UTF-8 and cannot be shell-quoted: {lossy:?}")]
    #[diagnostic(help(
        "this typically means the config-file directory is itself non-UTF-8; omit `--dry-run` (and `-q`) to skip rendering, or run from a UTF-8 directory"
    ))]
    CwdNotUtf8 {
        /// Lossy rendering of the offending path, for diagnostic
        /// purposes only — never emitted into the dry-run line.
        lossy: String,
    },

    /// A resolved cwd path contains a NUL byte and so cannot be
    /// shell-quoted for `--dry-run` (or the pre-exec preview). KDL
    /// string escapes (`\u{0}`) make this reachable from user input,
    /// even though a Unix filesystem path itself cannot contain a
    /// NUL. Execution would also fail when `Command::current_dir`
    /// builds its `CString`, but we surface the error earlier and
    /// with a clearer message via this variant.
    #[error("cwd path contains a NUL byte and cannot be used: {path:?}")]
    #[diagnostic(help(
        "remove the embedded NUL (typically a `\\u{{0}}` escape) from the `cwd=` value in your jig.kdl"
    ))]
    CwdContainsNul {
        /// The offending path as written / resolved.
        path: String,
    },

    /// A `cwd=` property was found at runtime to point at a directory
    /// that cannot be used as the child's working directory (does not
    /// exist, not a directory, permission denied, …). Detected when
    /// the spawn fails after `Command::current_dir` was set. Maps to
    /// exit code 125 per `SPEC.md` §3.5 / §2.12.
    #[error("could not enter cwd {path:?}: {source}")]
    #[diagnostic(help(
        "check that the path exists, is a directory, and is reachable; relative paths in `cwd=` resolve against the config-file directory"
    ))]
    CwdNotUsable {
        /// The resolved path (after applying the config-file-directory
        /// anchor, if relative).
        path: PathBuf,
        /// The underlying I/O error from the spawn attempt.
        #[source]
        source: std::io::Error,
    },

    /// A profile node carries a KDL property other than `extends`
    /// or `cwd`. Per `SPEC.md` §2.8.5 and §2.12 these are the only
    /// recognised properties on a profile node; every other property
    /// is rejected at parse time.
    #[error("unsupported property {name:?} on profile node")]
    #[diagnostic(help(
        "the allowed properties on a profile node are `extends=\"<parent>\"` (parent for inheritance) and `cwd=\"<path>\"` (working directory); remove this property or rename it"
    ))]
    UnsupportedPropertyOnProfile {
        /// The offending property name.
        name: String,
        /// Source the span points into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the offending property.
        #[label("not a recognised property")]
        span: SourceSpan,
    },

    /// A profile node's `extends` property carries a non-string
    /// value. The parent profile is named by a single string.
    #[error("`extends` value must be a string profile name")]
    #[diagnostic(help("write `extends=\"parent-profile\"` to inherit from `parent-profile`"))]
    ProfileExtendsBadValue {
        /// Source the span points into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the offending value.
        #[label("not a string")]
        span: SourceSpan,
    },

    /// A profile node has more than one `extends` property. v1
    /// supports single-parent inheritance only.
    #[error("profile has `extends` specified more than once")]
    #[diagnostic(help(
        "a profile may inherit from at most one parent in v1; remove the duplicate `extends`"
    ))]
    DuplicateProfileExtends {
        /// Source the spans point into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the first `extends` site.
        #[label("first `extends` here")]
        first: SourceSpan,
        /// Span of the second `extends` site.
        #[label("also here")]
        second: SourceSpan,
    },

    /// A profile's `extends="<parent>"` names a profile that does not
    /// exist in the same command. Per `SPEC.md` §2.8.5 / §2.9, the
    /// parent must be a defined profile within the same command;
    /// cross-command inheritance is not supported in v1.
    #[error("profile {profile:?} extends unknown profile {parent:?} in command {command:?}")]
    ProfileExtendsUnknownParent {
        /// The child profile whose `extends` was unresolved.
        profile: String,
        /// The parent name as written in the `extends` value.
        parent: String,
        /// The owning command.
        command: String,
        /// Source the span points into.
        #[source_code]
        src: NamedSource<String>,
        /// Span of the offending `extends=` value entry.
        #[label("not a profile in this command")]
        span: SourceSpan,
        /// Pre-formatted help listing the available sibling profile
        /// names and an optional did-you-mean suggestion.
        #[help]
        help: String,
    },

    /// The inheritance graph for one command contains a cycle. Per
    /// `SPEC.md` §2.8.5, the `extends` relation must be acyclic so
    /// resolution terminates. The `cycle` field lists profile names
    /// in order with the first name repeated at the end (e.g.
    /// `["A", "B", "A"]` for the two-cycle `A → B → A`); `spans`
    /// holds one `extends=` source span per edge on the cycle.
    #[error("profile inheritance cycle in command {command:?}: {}", cycle.join(" → "))]
    #[diagnostic(help(
        "break the cycle by removing one of the `extends=` pointers or pointing it at a non-cyclic profile"
    ))]
    ProfileInheritanceCycle {
        /// The owning command.
        command: String,
        /// Profile names on the cycle, with the start repeated at the
        /// end for readability (e.g. `["A","B","A"]`).
        cycle: Vec<String>,
        /// Source the spans point into.
        #[source_code]
        src: NamedSource<String>,
        /// One source span per edge on the cycle, in the same order
        /// as `cycle[..cycle.len()-1]`'s outgoing `extends=` values.
        #[label(collection, "on the cycle")]
        spans: Vec<SourceSpan>,
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

    /// The CLI invocation named a command/alias that does not
    /// appear in the config. CLI-origin error: no source span,
    /// per `SPEC.md` §7.4. The `help` field is pre-formatted with
    /// any did-you-mean suggestion.
    #[error("unknown command or alias {name:?}")]
    UnknownCommand {
        /// The unrecognized name as typed.
        name: String,
        /// Pre-formatted help with available names and an
        /// optional did-you-mean suggestion.
        #[help]
        help: String,
    },

    /// The CLI invocation named a command name that appears more
    /// than once in the config. Per `SPEC.md` §2.9 such names are
    /// not valid lookup keys — the user must invoke via one of the
    /// aliases. CLI-origin error: no source span.
    #[error("command name {name:?} is ambiguous")]
    AmbiguousCommand {
        /// The ambiguous command name as typed.
        name: String,
        /// Pre-formatted help listing the aliases of the duplicate
        /// entries.
        #[help]
        help: String,
    },

    /// The CLI invocation named a profile that does not appear
    /// under the matched command. CLI-origin error: no source
    /// span, per `SPEC.md` §7.4.
    #[error("unknown profile {profile:?} for command {command:?}")]
    UnknownProfile {
        /// The unrecognized profile name.
        profile: String,
        /// The matched command name.
        command: String,
        /// Pre-formatted help with available profiles and an
        /// optional did-you-mean suggestion.
        #[help]
        help: String,
    },

    /// `jig` was invoked without a command (and without `--list`
    /// or another flag-only mode). Per Q2, this is treated as a
    /// usage error: `main` prints `--help` to stderr and exits
    /// 125 directly, bypassing the standard miette rendering.
    #[error("no command specified")]
    MissingCommand,

    /// A pass-through argument is not valid UTF-8, so it cannot
    /// be shell-quoted for `--dry-run` output. Per Q7, we error
    /// rather than emit replacement characters that would break
    /// the §7.2 copy-paste contract.
    #[error("pass-through argument is not valid UTF-8 and cannot be shell-quoted: {lossy:?}")]
    #[diagnostic(help(
        "remove the non-UTF-8 argument or omit `--dry-run`; execution itself does not require UTF-8"
    ))]
    PassthroughNotUtf8 {
        /// Lossy rendering of the offending value, for diagnostic
        /// purposes only — never emitted into the dry-run line.
        lossy: String,
    },

    /// A value contains a NUL byte and so cannot be passed to a
    /// process or shell-quoted. Rare but defensively handled.
    #[error("argument {value:?} contains a NUL byte and cannot be passed to a process")]
    ArgumentContainsNul {
        /// The offending value (with the NUL byte present).
        value: String,
    },

    /// The resolved program was not found on `$PATH` or as a path.
    /// Maps to exit code 127 per `SPEC.md` §3.5.
    #[error("command not found: {program:?}")]
    #[diagnostic(help(
        "check that the command name in jig.kdl matches an executable on $PATH or a path on disk"
    ))]
    ExecNotFound {
        /// The program name as it appeared in the config.
        program: String,
    },

    /// The resolved program was found but cannot be executed
    /// (permission denied, not a regular file, ...). Maps to exit
    /// code 126 per `SPEC.md` §3.5.
    #[error("command is not executable: {program:?}")]
    #[diagnostic(help("check the file's mode bits (e.g. `chmod +x`)"))]
    ExecNotExecutable {
        /// The program name as it appeared in the config.
        program: String,
        /// The underlying I/O error from the spawn attempt.
        #[source]
        source: std::io::Error,
    },

    /// Spawning the child process failed for a reason other than
    /// "not found" or "not executable". Maps to exit code 125
    /// (`jig`-internal failure) since this is unusual and likely a
    /// bug or system issue rather than a user mistake.
    #[error("failed to spawn {program:?}: {source}")]
    ExecSpawnFailed {
        /// The program name as it appeared in the config.
        program: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

impl Error {
    /// Wrapper-tool exit code for this error per `SPEC.md` §3.5.
    /// Most variants are `jig`-internal failures (125); exec-layer
    /// errors map to 126 / 127.
    #[must_use]
    pub const fn exit_code(&self) -> ExitCode {
        match self {
            Self::ExecNotFound { .. } => ExitCode::NotFound,
            Self::ExecNotExecutable { .. } => ExitCode::NotExecutable,
            Self::ConfigNotFound { .. }
            | Self::ConfigIo { .. }
            | Self::KdlParse(_)
            | Self::FlagMultipleValues { .. }
            | Self::NodeHasProperties { .. }
            | Self::ExpectedString { .. }
            | Self::LeadingDashName { .. }
            | Self::LeadingPlusName { .. }
            | Self::NullWithAppendMarker { .. }
            | Self::EmptyKeyAfterMarker { .. }
            | Self::UnknownTypeAnnotation { .. }
            | Self::EnvOnNodeWithChildren { .. }
            | Self::EnvNoValue { .. }
            | Self::EnvMultipleValues { .. }
            | Self::EnvInvalidValue { .. }
            | Self::EnvWithAppendMarker { .. }
            | Self::EnvNameInvalid { .. }
            | Self::DuplicateEnvName { .. }
            | Self::DuplicateCommandWithoutAlias { .. }
            | Self::DuplicateAlias { .. }
            | Self::CommandAliasCollision { .. }
            | Self::CwdBadValue { .. }
            | Self::DuplicateCwd { .. }
            | Self::CwdNotUtf8 { .. }
            | Self::CwdContainsNul { .. }
            | Self::CwdNotUsable { .. }
            | Self::UnsupportedPropertyOnProfile { .. }
            | Self::ProfileExtendsBadValue { .. }
            | Self::DuplicateProfileExtends { .. }
            | Self::ProfileExtendsUnknownParent { .. }
            | Self::ProfileInheritanceCycle { .. }
            | Self::DuplicateProfile { .. }
            | Self::UnknownCommand { .. }
            | Self::AmbiguousCommand { .. }
            | Self::UnknownProfile { .. }
            | Self::MissingCommand
            | Self::PassthroughNotUtf8 { .. }
            | Self::ArgumentContainsNul { .. }
            | Self::ExecSpawnFailed { .. } => ExitCode::JigFailure,
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
            searched: "jig.kdl, .jig.kdl".to_string(),
            from: PathBuf::from("/home/user/project/src"),
            up_to: PathBuf::from("/home/user"),
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
    fn duplicate_command_without_alias_renders() {
        let src = NamedSource::new("jig.kdl", "llama-server {}\nllama-server {}\n".to_string());
        let err = Error::DuplicateCommandWithoutAlias {
            name: "llama-server".to_string(),
            src,
            first: SourceSpan::from((0, 12)),
            second: SourceSpan::from((16, 12)),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn ambiguous_command_renders() {
        let err = Error::AmbiguousCommand {
            name: "llama-server".to_string(),
            help: "command name \"llama-server\" appears more than once; \
                   invoke via one of its aliases: serve-coder, serve-chat"
                .to_string(),
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
    fn unknown_command_renders() {
        let err = Error::UnknownCommand {
            name: "qwen-codr".to_string(),
            help: "available commands: llama-server, gemma-server, serve\ndid you mean \"serve\"?"
                .to_string(),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn unknown_profile_renders() {
        let err = Error::UnknownProfile {
            profile: "qwen-codr".to_string(),
            command: "llama-server".to_string(),
            help: "available profiles: qwen-coder, llama3\ndid you mean \"qwen-coder\"?"
                .to_string(),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn passthrough_not_utf8_renders() {
        let err = Error::PassthroughNotUtf8 {
            lossy: "hello\u{fffd}world".to_string(),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn config_not_found_maps_to_jig_failure() {
        let err = Error::ConfigNotFound {
            searched: String::new(),
            from: PathBuf::from("/"),
            up_to: PathBuf::from("/"),
        };
        assert_eq!(err.exit_code(), ExitCode::JigFailure);
    }

    #[test]
    fn exec_not_found_maps_to_127() {
        let err = Error::ExecNotFound {
            program: "missing-bin".to_string(),
        };
        assert_eq!(err.exit_code(), ExitCode::NotFound);
        assert_eq!(err.exit_code().as_i32(), 127);
    }

    #[test]
    fn exec_not_executable_maps_to_126() {
        let err = Error::ExecNotExecutable {
            program: "bad-bin".to_string(),
            source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        };
        assert_eq!(err.exit_code(), ExitCode::NotExecutable);
        assert_eq!(err.exit_code().as_i32(), 126);
    }

    #[test]
    fn exec_not_found_renders() {
        let err = Error::ExecNotFound {
            program: "llama-server".to_string(),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn unknown_type_annotation_renders() {
        let src = NamedSource::new("jig.kdl", "foo {\n  (cwd)dir \"/x\"\n}\n".to_string());
        let err = Error::UnknownTypeAnnotation {
            annotation: "cwd".to_string(),
            src,
            span: SourceSpan::from((9, 3)),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn env_no_value_renders() {
        let src = NamedSource::new("jig.kdl", "foo {\n  (env)BARE\n}\n".to_string());
        let err = Error::EnvNoValue {
            src,
            span: SourceSpan::from((13, 4)),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn env_invalid_value_renders() {
        let src = NamedSource::new("jig.kdl", "foo {\n  (env)X #true\n}\n".to_string());
        let err = Error::EnvInvalidValue {
            src,
            span: SourceSpan::from((15, 5)),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn null_with_append_marker_renders() {
        let src = NamedSource::new("jig.kdl", "foo {\n  +a #null\n}\n".to_string());
        let err = Error::NullWithAppendMarker {
            src,
            span: SourceSpan::from((8, 2)),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn env_with_append_marker_renders() {
        let src = NamedSource::new("jig.kdl", "foo {\n  (env)+X \"y\"\n}\n".to_string());
        let err = Error::EnvWithAppendMarker {
            src,
            span: SourceSpan::from((13, 2)),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn env_name_invalid_renders() {
        let src = NamedSource::new(
            "jig.kdl",
            "foo {\n  (env)\"FOO-BAR\" \"x\"\n}\n".to_string(),
        );
        let err = Error::EnvNameInvalid {
            name: "FOO-BAR".to_string(),
            src,
            span: SourceSpan::from((13, 9)),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn duplicate_env_name_renders() {
        let src = NamedSource::new(
            "jig.kdl",
            "foo {\n  (env)X \"1\"\n  (env)X \"2\"\n}\n".to_string(),
        );
        let err = Error::DuplicateEnvName {
            name: "X".to_string(),
            scope: "command \"foo\" defaults".to_string(),
            src,
            first: SourceSpan::from((13, 1)),
            second: SourceSpan::from((25, 1)),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn unsupported_property_on_profile_renders() {
        let src = NamedSource::new(
            "jig.kdl",
            "foo {\n  child base=\"parent\" {}\n}\n".to_string(),
        );
        let err = Error::UnsupportedPropertyOnProfile {
            name: "base".to_string(),
            src,
            span: SourceSpan::from((14, 13)),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn profile_extends_bad_value_renders() {
        let src = NamedSource::new(
            "jig.kdl",
            "foo {\n  child extends=#true {}\n}\n".to_string(),
        );
        let err = Error::ProfileExtendsBadValue {
            src,
            span: SourceSpan::from((22, 5)),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn duplicate_profile_extends_renders() {
        let src = NamedSource::new(
            "jig.kdl",
            "foo {\n  child extends=\"a\" extends=\"b\" {}\n}\n".to_string(),
        );
        let err = Error::DuplicateProfileExtends {
            src,
            first: SourceSpan::from((14, 11)),
            second: SourceSpan::from((26, 11)),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn profile_extends_unknown_parent_renders() {
        let err = Error::ProfileExtendsUnknownParent {
            profile: "child".to_string(),
            parent: "paren".to_string(),
            command: "foo".to_string(),
            src: NamedSource::new(
                "jig.kdl",
                "foo {\n  parent {}\n  child extends=\"paren\" {}\n}\n".to_string(),
            ),
            span: SourceSpan::from((36, 7)),
            help: "available profiles: parent, child\ndid you mean \"parent\"?".to_string(),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn profile_inheritance_cycle_renders() {
        let err = Error::ProfileInheritanceCycle {
            command: "foo".to_string(),
            cycle: vec!["a".to_string(), "b".to_string(), "a".to_string()],
            src: NamedSource::new(
                "jig.kdl",
                "foo {\n  a extends=\"b\" {}\n  b extends=\"a\" {}\n}\n".to_string(),
            ),
            spans: vec![SourceSpan::from((18, 3)), SourceSpan::from((36, 3))],
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn cwd_bad_value_renders() {
        let src = NamedSource::new("jig.kdl", "foo cwd=42 {}\n".to_string());
        let err = Error::CwdBadValue {
            src,
            span: SourceSpan::from((8, 2)),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn duplicate_cwd_renders() {
        let src = NamedSource::new("jig.kdl", "foo cwd=\"/a\" cwd=\"/b\" {}\n".to_string());
        let err = Error::DuplicateCwd {
            src,
            first: SourceSpan::from((4, 8)),
            second: SourceSpan::from((13, 8)),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn cwd_not_utf8_renders() {
        let err = Error::CwdNotUtf8 {
            lossy: "/some/\u{fffd}/dir".to_string(),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn cwd_contains_nul_renders() {
        let err = Error::CwdContainsNul {
            path: "/some\u{0}/dir".to_string(),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn cwd_not_usable_renders() {
        let err = Error::CwdNotUsable {
            path: PathBuf::from("/this/does/not/exist"),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }

    #[test]
    fn env_on_node_with_children_renders() {
        let src = NamedSource::new(
            "jig.kdl",
            "foo {\n  (env)X \"y\" {\n    inner\n  }\n}\n".to_string(),
        );
        let err = Error::EnvOnNodeWithChildren {
            src,
            span: SourceSpan::from((13, 1)),
        };
        insta::assert_snapshot!(render_for_snapshot(&err));
    }
}
