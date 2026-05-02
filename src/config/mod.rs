//! Parsed-and-validated configuration tree.
//!
//! Types defined here mirror `IMPLEMENTATION.md` §7.2 and §7.3.1
//! verbatim. They are produced by [`parse::parse_str`] and
//! consumed by `--list` rendering today; later steps add the
//! validator and resolver as additional consumers.

pub mod load;
pub mod parse;

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
    /// Optional alias (the first KDL value on the command node).
    pub alias: Option<String>,
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
