//! Parsed-and-validated configuration tree.
//!
//! Types defined here mirror `IMPLEMENTATION.md` §7.2 and §7.3.1
//! verbatim, with the addition of source spans on each name-bearing
//! field so the validator can produce two-span diagnostics per
//! `SPEC.md` §7.4 (e.g. duplicate alias pointing at both sites).
//! Spans use `miette::SourceSpan`; they are populated by the parser
//! and read by the validator.

use miette::SourceSpan;

pub mod load;
pub mod parse;
pub mod validate;

/// A whole `jig.kdl` document, post-parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Top-level command entries, in source order.
    pub commands: Vec<Command>,
}

/// One top-level command entry — the executable to run, an optional
/// alias, and the source-ordered children that contribute defaults
/// and profiles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// The executable name (resolved via `$PATH`) or path. The KDL
    /// node name.
    pub name: String,
    /// Source span of `name` in the KDL document.
    pub name_span: SourceSpan,
    /// Optional alias (the first KDL value on the command node).
    pub alias: Option<String>,
    /// Source span of the alias entry, present iff `alias` is.
    pub alias_span: Option<SourceSpan>,
    /// Source-ordered children: defaults and profiles interleaved.
    /// See `IMPLEMENTATION.md` §7.3.1 for why this is one list
    /// rather than two collections.
    pub children: Vec<CommandChild>,
}

/// One child of a command node — either a default argument or a
/// named profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandChild {
    /// A default argument: a flag or positional that contributes
    /// regardless of which profile is selected (`SPEC.md` §2.7).
    Default(Argument),
    /// A named profile, with its own ordered argument list.
    /// Profiles do not nest within profiles in v1 (`SPEC.md` §2.3).
    Profile {
        /// Profile name (KDL node name).
        name: String,
        /// Source span of `name`.
        name_span: SourceSpan,
        /// Profile body, in source order.
        args: Vec<Argument>,
    },
}

/// One argument in either a command's default list or a profile body.
/// The structural distinction between flags and positionals is fixed
/// at parse time per `SPEC.md` §2.4.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Argument {
    /// A KDL node with exactly one value: emitted as `<key> <value>`
    /// on the resolved command line (with key prefix per §2.5).
    Flag {
        /// Flag key, possibly with explicit dash prefix.
        key: FlagKey,
        /// Source span of the key (the KDL node name).
        key_span: SourceSpan,
        /// Flag value (boolean or stringified literal).
        value: FlagValue,
    },
    /// A KDL node with no value: the node name is the literal
    /// positional value (`SPEC.md` §2.6).
    Positional(String),
}

/// Flag key, distinguishing keys that need prefix synthesis from
/// keys passed verbatim. Per `SPEC.md` §2.5.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlagKey {
    /// Bare key (no leading dash in the source). Gets `-` if it is
    /// exactly one character, `--` otherwise.
    Inferred(String),
    /// Key written with an explicit `-` or `--` prefix in the
    /// source. Passed through unchanged.
    Verbatim(String),
}

impl FlagKey {
    /// Resolve this key to its CLI form per `SPEC.md` §2.5: a
    /// `Verbatim` key is passed through; an `Inferred` key gets a
    /// `-` prefix when one character long, `--` otherwise.
    ///
    /// Used by the validator (for §2.9 duplicate detection) and the
    /// formatter (for emission). We measure length in `char`s, not
    /// bytes, so a multi-byte single character also picks up `-`.
    #[must_use]
    pub fn to_cli_flag(&self) -> String {
        match self {
            Self::Verbatim(s) => s.clone(),
            Self::Inferred(s) if s.chars().count() == 1 => format!("-{s}"),
            Self::Inferred(s) => format!("--{s}"),
        }
    }
}

/// Flag value, distinguishing the KDL boolean keyword `#true`/`#false`
/// (which control include/suppress) from any other literal value.
/// Per `SPEC.md` §2.4.1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlagValue {
    /// The KDL boolean keyword. `true` → emit the flag with no
    /// value; `false` → suppress the flag entirely.
    Bool(bool),
    /// Any non-boolean value, stored as the textual representation
    /// that should appear on the command line. We keep the original
    /// source text rather than parsed numeric form to avoid
    /// precision loss on floats and integer-vs-float ambiguity.
    Literal(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_key_to_cli_long_form() {
        assert_eq!(
            FlagKey::Inferred("host".to_string()).to_cli_flag(),
            "--host"
        );
    }

    #[test]
    fn flag_key_to_cli_short_form() {
        assert_eq!(FlagKey::Inferred("m".to_string()).to_cli_flag(), "-m");
    }

    #[test]
    fn flag_key_to_cli_verbatim_passes_through() {
        assert_eq!(FlagKey::Verbatim("-ngl".to_string()).to_cli_flag(), "-ngl");
        assert_eq!(
            FlagKey::Verbatim("--explicit".to_string()).to_cli_flag(),
            "--explicit"
        );
    }
}
