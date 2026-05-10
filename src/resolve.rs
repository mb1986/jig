//! Resolve a CLI invocation against a parsed [`Config`] into the
//! candidate argument list per `SPEC.md` §2.8 and §4.
//!
//! The algorithm:
//!
//! 1. Look up the named command by name, then by alias. Return
//!    [`Error::UnknownCommand`] (with did-you-mean) on miss.
//! 2. If a profile name was supplied, look it up within the matched
//!    command. Return [`Error::UnknownProfile`] (with did-you-mean
//!    and an "available" list) on miss.
//! 3. Walk the command's children in source order. Defaults push
//!    one candidate each; the **selected** profile contributes its
//!    own children (in source order); other profiles are skipped
//!    (`SPEC.md` §2.7).
//! 4. Group flag candidates by resolved CLI form (per §2.5). For
//!    each key build an emission plan per the per-key resolution in
//!    `SPEC.md` §2.8:
//!    - Suppression: profile-side `#false` (regardless of `+`
//!      marker) drops *all* default occurrences of the key and
//!      drops the `#false` entries themselves. Default-side `#false`
//!      drops just that occurrence.
//!    - Partition surviving entries into unmarked default,
//!      unmarked profile, and marked (`+` prefix on either side).
//!    - Marked entries always emit at their own source position
//!      with their own value.
//!    - Unmarked entries fall into single mode (≤ 1 on each side
//!      → v1 first-occurrence positioning with profile-value
//!      precedence) or repeat mode (otherwise → emit each at its
//!      own source position).
//! 5. Resolve env-var contributions on a parallel channel
//!    (`SPEC.md` §2.11): walk the command's defaults env list, then
//!    the selected profile's env list; for each name pick at most
//!    one outcome — profile-side `Unset` wins over profile-side
//!    `Set` wins over default-side `Unset` wins over default-side
//!    `Set`. The outcome is emitted at the first-occurrence walk
//!    position so `--list` and `--dry-run` order is deterministic.
//!
//! Positionals are not subject to override or suppression and are
//! emitted at their walk position in source order.

use std::collections::HashMap;

use crate::config::{
    Argument, Command, CommandChild, Config, EnvEntry, EnvValue, FlagMode, FlagValue,
};
use crate::errors::{Error, Result};

/// Output of a successful resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The program to execute (the command's `name` field).
    pub program: String,
    /// The candidate list after walk + collapse + suppression.
    pub args: Vec<Argument>,
    /// Env-var outcomes to apply to the spawned child, in
    /// first-occurrence walk order (`SPEC.md` §2.11).
    pub env: Vec<EnvOp>,
}

/// One env-var operation to apply to the spawned child process via
/// `Command::env` / `Command::env_remove`. Per `SPEC.md` §2.11 /
/// §3.6.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvOp {
    /// Call `Command::env(name, value)`.
    Set {
        /// Variable name.
        name: String,
        /// Value to assign on the child.
        value: String,
    },
    /// Call `Command::env_remove(name)`.
    Unset {
        /// Variable name.
        name: String,
    },
}

/// Resolve `name` and optional `profile` against `config`.
///
/// # Errors
///
/// Returns [`Error::UnknownCommand`] if no command matches the
/// name or alias, or [`Error::UnknownProfile`] if `profile` does
/// not exist on the matched command.
pub fn resolve(config: &Config, name: &str, profile: Option<&str>) -> Result<Resolved> {
    let cmd = lookup_command(config, name)?;
    let selected_profile = match profile {
        None => None,
        Some(p) => Some(lookup_profile(cmd, p)?),
    };

    // Step 1: walk children, tagging each candidate with its origin
    // (default vs selected-profile).
    let mut candidates: Vec<(Argument, bool /* from_profile */)> = Vec::new();
    for child in &cmd.children {
        match child {
            CommandChild::Default(arg) => candidates.push((arg.clone(), false)),
            CommandChild::Profile { name, args, .. } if Some(name.as_str()) == selected_profile => {
                for arg in args {
                    candidates.push((arg.clone(), true));
                }
            }
            CommandChild::Profile { .. } => {}
        }
    }

    // Resolve env-var contributions on a parallel channel.
    let profile_env: &[EnvEntry] = selected_profile
        .and_then(|p| {
            cmd.children.iter().find_map(|c| match c {
                CommandChild::Profile { name, env, .. } if name == p => Some(env.as_slice()),
                _ => None,
            })
        })
        .unwrap_or(&[]);
    let env = resolve_env(&cmd.env, profile_env);

    // Step 2: group flag candidates by resolved CLI form, then
    // compute the emission plan per key. The plan records, for each
    // emitting source index, the value to use at that position;
    // indices not present are suppressed (whether by collapse or by
    // `#false`).
    let mut by_key: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, (arg, _)) in candidates.iter().enumerate() {
        if let Argument::Flag { key, .. } = arg {
            by_key.entry(key.to_cli_flag()).or_default().push(i);
        }
    }
    let mut emit_value: HashMap<usize, FlagValue> = HashMap::new();
    for indices in by_key.values() {
        plan_key(&candidates, indices, &mut emit_value);
    }

    // Step 3: assemble. Positionals always emit; flags emit iff they
    // appear in the plan, taking the planned value (which is what
    // makes profile override work in single mode).
    let mut out: Vec<Argument> = Vec::with_capacity(candidates.len());
    for (i, (arg, _)) in candidates.into_iter().enumerate() {
        match arg {
            Argument::Positional(_) => out.push(arg),
            Argument::Flag {
                key,
                key_span,
                value: _,
                mode,
            } => {
                if let Some(value) = emit_value.remove(&i) {
                    out.push(Argument::Flag {
                        key,
                        key_span,
                        value,
                        mode,
                    });
                }
            }
        }
    }

    Ok(Resolved {
        program: cmd.name.clone(),
        args: out,
        env,
    })
}

/// Resolve env-var outcomes per `SPEC.md` §2.11. Walks
/// `defaults_env` then `profile_env`; for each distinct name,
/// profile-side wins over default-side and `Unset` wins over `Set`
/// on the same side. The outcome is emitted at the first-occurrence
/// walk position.
fn resolve_env(defaults_env: &[EnvEntry], profile_env: &[EnvEntry]) -> Vec<EnvOp> {
    // First-occurrence ordering: stable index of first appearance
    // per name across the (defaults, profile) walk.
    let mut first_index: HashMap<&str, usize> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    for entry in defaults_env.iter().chain(profile_env.iter()) {
        if !first_index.contains_key(entry.name.as_str()) {
            first_index.insert(entry.name.as_str(), order.len());
            order.push(entry.name.as_str());
        }
    }

    // Per-name resolution. For each name, pick the winning outcome
    // by the precedence in §2.11.
    let mut out: Vec<EnvOp> = Vec::with_capacity(order.len());
    for name in order {
        let outcome = pick_env_outcome(name, defaults_env, profile_env);
        out.push(outcome);
    }
    out
}

fn pick_env_outcome(name: &str, defaults: &[EnvEntry], profile: &[EnvEntry]) -> EnvOp {
    // Profile-side `Unset` wins over everything.
    if profile
        .iter()
        .any(|e| e.name == name && matches!(e.value, EnvValue::Unset))
    {
        return EnvOp::Unset {
            name: name.to_string(),
        };
    }
    // Then profile-side `Set`. Validation enforces per-scope
    // uniqueness, so at most one entry can match.
    if let Some(value) = profile
        .iter()
        .find_map(|e| match (&e.value, e.name == name) {
            (EnvValue::Set(v), true) => Some(v.clone()),
            _ => None,
        })
    {
        return EnvOp::Set {
            name: name.to_string(),
            value,
        };
    }
    // Then default-side `Unset`.
    if defaults
        .iter()
        .any(|e| e.name == name && matches!(e.value, EnvValue::Unset))
    {
        return EnvOp::Unset {
            name: name.to_string(),
        };
    }
    // Otherwise default-side `Set` (validation guarantees per-scope
    // uniqueness, so at most one such entry exists).
    let value = defaults
        .iter()
        .find_map(|e| match (&e.value, e.name == name) {
            (EnvValue::Set(v), true) => Some(v.clone()),
            _ => None,
        })
        .expect("invariant: name was added to `order` from one of the two slices");
    EnvOp::Set {
        name: name.to_string(),
        value,
    }
}

/// Per-key resolution per `SPEC.md` §2.8. Reads the candidates at
/// `indices` (all sharing a resolved CLI key), applies suppression
/// and the single/repeat/marker rules, and writes the resulting
/// emission decisions into `emit_value`.
fn plan_key(
    candidates: &[(Argument, bool)],
    indices: &[usize],
    emit_value: &mut HashMap<usize, FlagValue>,
) {
    // Materialise each candidate's relevant fields. Borrow only;
    // values are cloned at write time.
    struct Entry<'a> {
        idx: usize,
        from_profile: bool,
        value: &'a FlagValue,
        mode: FlagMode,
    }
    let entries: Vec<Entry<'_>> = indices
        .iter()
        .map(|&i| {
            let (arg, from_profile) = &candidates[i];
            let Argument::Flag { value, mode, .. } = arg else {
                unreachable!("invariant: by_key only references flag candidates");
            };
            Entry {
                idx: i,
                from_profile: *from_profile,
                value,
                mode: *mode,
            }
        })
        .collect();

    // Suppression. Profile-side `#false` (any marker) clears every
    // default contribution for this key; default-side `#false` only
    // drops itself.
    let profile_has_false = entries
        .iter()
        .any(|e| e.from_profile && matches!(e.value, FlagValue::Bool(false)));
    let surviving: Vec<&Entry<'_>> = if profile_has_false {
        entries
            .iter()
            .filter(|e| e.from_profile && !matches!(e.value, FlagValue::Bool(false)))
            .collect()
    } else {
        entries
            .iter()
            .filter(|e| !matches!(e.value, FlagValue::Bool(false)))
            .collect()
    };

    // Marked entries always emit at their own position, regardless
    // of unmarked-side mode.
    for e in surviving.iter().filter(|e| e.mode == FlagMode::Append) {
        emit_value.insert(e.idx, e.value.clone());
    }

    // Unmarked entries decide single-mode vs repeat-mode.
    let d_unmarked: Vec<&&Entry<'_>> = surviving
        .iter()
        .filter(|e| !e.from_profile && e.mode == FlagMode::Plain)
        .collect();
    let p_unmarked: Vec<&&Entry<'_>> = surviving
        .iter()
        .filter(|e| e.from_profile && e.mode == FlagMode::Plain)
        .collect();

    if d_unmarked.len() <= 1 && p_unmarked.len() <= 1 {
        // Single mode (v1 first-occurrence positioning). Position is
        // the default's source index when present, else the
        // profile's; value is the profile's when present, else the
        // default's.
        let pos_idx = if let Some(d) = d_unmarked.first() {
            d.idx
        } else if let Some(p) = p_unmarked.first() {
            p.idx
        } else {
            return;
        };
        let value = p_unmarked
            .first()
            .map_or_else(|| d_unmarked[0].value.clone(), |p| p.value.clone());
        emit_value.insert(pos_idx, value);
    } else {
        // Repeat mode: every unmarked occurrence emits in place.
        for e in d_unmarked.iter().chain(p_unmarked.iter()) {
            emit_value.insert(e.idx, e.value.clone());
        }
    }
}

/// Look up a command for completion-candidate emission. Mirrors the
/// rules in [`lookup_command`] but never errors: returns `None` for
/// unknown names and for duplicated bare names (which have no unique
/// profile set — the user must invoke via an alias). Used by
/// [`crate::complete`].
#[must_use]
pub fn find_for_completion<'a>(config: &'a Config, name: &str) -> Option<&'a Command> {
    let name_matches: Vec<&Command> = config.commands.iter().filter(|c| c.name == name).collect();
    if name_matches.len() == 1 {
        return Some(name_matches[0]);
    }
    if name_matches.len() > 1 {
        // Duplicated bare name → ambiguous, no unique profile set.
        return None;
    }
    config
        .commands
        .iter()
        .find(|c| c.alias.as_deref() == Some(name))
}

fn lookup_command<'a>(config: &'a Config, name: &str) -> Result<&'a Command> {
    // Step 1: count name matches.
    let name_matches: Vec<&Command> = config.commands.iter().filter(|c| c.name == name).collect();

    if name_matches.len() == 1 {
        return Ok(name_matches[0]);
    }
    if name_matches.len() > 1 {
        // Validation guarantees every duplicated occurrence has an
        // alias; list them so the user can pick one.
        let aliases: Vec<&str> = name_matches
            .iter()
            .map(|c| {
                c.alias.as_deref().expect(
                    "invariant: validation requires every duplicated command name to have an alias",
                )
            })
            .collect();
        return Err(Error::AmbiguousCommand {
            name: name.to_string(),
            help: format!(
                "command name {name:?} appears more than once; invoke via one of its aliases: {}",
                aliases.join(", ")
            ),
        });
    }

    // Step 2: alias lookup. Validation enforces alias uniqueness so
    // at most one match is possible.
    if let Some(cmd) = config
        .commands
        .iter()
        .find(|c| c.alias.as_deref() == Some(name))
    {
        return Ok(cmd);
    }

    // Step 3: unknown. Build the did-you-mean candidate list from
    // names that are valid lookup keys (single-occurrence names) plus
    // every alias.
    let mut name_counts: HashMap<&str, usize> = HashMap::new();
    for cmd in &config.commands {
        *name_counts.entry(cmd.name.as_str()).or_insert(0) += 1;
    }
    let mut all: Vec<&str> = Vec::new();
    for cmd in &config.commands {
        if name_counts[cmd.name.as_str()] == 1 {
            all.push(&cmd.name);
        }
        if let Some(alias) = &cmd.alias {
            all.push(alias);
        }
    }
    let suggestion = nearest(name, &all);
    Err(Error::UnknownCommand {
        name: name.to_string(),
        help: build_help("commands", &all, suggestion),
    })
}

fn lookup_profile<'a>(cmd: &'a Command, profile: &'a str) -> Result<&'a str> {
    for child in &cmd.children {
        if let CommandChild::Profile { name, .. } = child
            && name == profile
        {
            return Ok(name);
        }
    }
    let available: Vec<&str> = cmd
        .children
        .iter()
        .filter_map(|c| match c {
            CommandChild::Profile { name, .. } => Some(name.as_str()),
            CommandChild::Default(_) => None,
        })
        .collect();
    let suggestion = nearest(profile, &available);
    Err(Error::UnknownProfile {
        profile: profile.to_string(),
        command: cmd.name.clone(),
        help: build_help("profiles", &available, suggestion),
    })
}

/// Format the `help` field rendered with each diagnostic.
fn build_help(label: &str, available: &[&str], did_you_mean: Option<&str>) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    if available.is_empty() {
        let _ = write!(s, "no {label} are defined in this config");
    } else {
        let _ = write!(s, "available {label}: {}", available.join(", "));
    }
    if let Some(suggestion) = did_you_mean {
        s.push('\n');
        let _ = write!(s, "did you mean {suggestion:?}?");
    }
    s
}

/// Return the nearest entry in `haystack` to `needle` by edit
/// distance, if any are within a small threshold.
fn nearest<'a>(needle: &str, haystack: &[&'a str]) -> Option<&'a str> {
    let threshold = 2.max(needle.chars().count() / 3);
    haystack
        .iter()
        .map(|s| (*s, levenshtein(needle, s)))
        .filter(|(_, d)| *d <= threshold)
        .min_by_key(|(_, d)| *d)
        .map(|(s, _)| s)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur: Vec<usize> = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FlagKey, FlagValue, parse::parse_str};

    fn parse(input: &str) -> Config {
        parse_str(input, "test.kdl").expect("invariant: test KDL must parse")
    }

    /// Convenience: extract `(resolved_key, value_string)` pairs and
    /// positionals as `("", value)` so the order can be asserted in
    /// one Vec.
    fn flatten(resolved: &Resolved) -> Vec<(String, String)> {
        resolved
            .args
            .iter()
            .map(|a| match a {
                Argument::Flag { key, value, .. } => {
                    let v = match value {
                        FlagValue::Bool(true) => String::new(),
                        FlagValue::Bool(false) => "<#false>".to_string(),
                        FlagValue::Literal(s) => s.clone(),
                    };
                    (key.to_cli_flag(), v)
                }
                Argument::Positional(s) => (String::new(), s.clone()),
            })
            .collect()
    }

    // --- §3.1 / §4 lookup ---

    #[test]
    fn lookup_by_command_name() {
        let cfg = parse("llama-server \"serve\" {\n  host \"0.0.0.0\"\n}\n");
        let r = resolve(&cfg, "llama-server", None).unwrap();
        assert_eq!(r.program, "llama-server");
    }

    #[test]
    fn lookup_by_alias() {
        let cfg = parse("llama-server \"serve\" {\n  host \"0.0.0.0\"\n}\n");
        let r = resolve(&cfg, "serve", None).unwrap();
        assert_eq!(r.program, "llama-server");
    }

    #[test]
    fn lookup_by_alias_when_name_is_duplicated() {
        let cfg = parse(
            r#"llama-server "serve1" {
                a #true
            }
            llama-server "serve2" {
                b #true
            }"#,
        );
        let r = resolve(&cfg, "serve1", None).unwrap();
        assert_eq!(r.program, "llama-server");
        assert_eq!(flatten(&r), vec![("-a".into(), String::new())]);

        let r = resolve(&cfg, "serve2", None).unwrap();
        assert_eq!(r.program, "llama-server");
        assert_eq!(flatten(&r), vec![("-b".into(), String::new())]);
    }

    #[test]
    fn bare_name_lookup_of_duplicated_command_is_ambiguous() {
        let cfg = parse(
            r#"llama-server "serve1" { a #true }
            llama-server "serve2" { b #true }"#,
        );
        let err = resolve(&cfg, "llama-server", None).unwrap_err();
        let Error::AmbiguousCommand { name, help } = err else {
            panic!("expected AmbiguousCommand");
        };
        assert_eq!(name, "llama-server");
        assert!(help.contains("serve1"));
        assert!(help.contains("serve2"));
    }

    #[test]
    fn ambiguous_help_lists_all_aliases_when_three_duplicates() {
        let cfg = parse(
            r#"foo "a" {}
            foo "b" {}
            foo "c" {}"#,
        );
        let err = resolve(&cfg, "foo", None).unwrap_err();
        let Error::AmbiguousCommand { help, .. } = err else {
            panic!("expected AmbiguousCommand");
        };
        assert!(help.contains('a'));
        assert!(help.contains('b'));
        assert!(help.contains('c'));
    }

    #[test]
    fn duplicated_name_excluded_from_did_you_mean() {
        // `foo` is duplicated, so a typo close to `foo` should not
        // suggest it — typing `foo` would have produced an ambiguous
        // error rather than a working invocation.
        let cfg = parse(
            r#"foo "f1" {}
            foo "f2" {}"#,
        );
        let err = resolve(&cfg, "fop", None).unwrap_err();
        let Error::UnknownCommand { help, .. } = err else {
            panic!("expected UnknownCommand");
        };
        assert!(
            !help.contains("\"foo\""),
            "did-you-mean should not suggest a duplicated bare name; got: {help}"
        );
    }

    #[test]
    fn unknown_command_with_did_you_mean() {
        let cfg = parse("llama-server {}\ngemma-server {}\n");
        let err = resolve(&cfg, "llama-servr", None).unwrap_err();
        let Error::UnknownCommand { name, help } = err else {
            panic!("expected UnknownCommand");
        };
        assert_eq!(name, "llama-servr");
        assert!(help.contains("llama-server"));
        assert!(help.contains("did you mean"));
    }

    #[test]
    fn unknown_profile_with_available_list() {
        let cfg = parse("foo {\n  fast {}\n  slow {}\n}\n");
        let err = resolve(&cfg, "foo", Some("medium")).unwrap_err();
        let Error::UnknownProfile {
            profile,
            command,
            help,
        } = err
        else {
            panic!();
        };
        assert_eq!(profile, "medium");
        assert_eq!(command, "foo");
        assert!(help.contains("fast"));
        assert!(help.contains("slow"));
    }

    // --- §5.1 llama-server example ---

    #[test]
    fn spec_5_1_no_profile_yields_defaults_only() {
        let cfg = parse(
            r#"llama-server "serve" {
                host "0.0.0.0"
                port 8090
                c 32768
                flash-attn #true
                qwen-coder {
                    m "/m1"
                    -ngl 999
                }
                llama3 {
                    m "/m2"
                    port 8091
                }
            }"#,
        );
        let r = resolve(&cfg, "serve", None).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("--host".into(), "0.0.0.0".into()),
                ("--port".into(), "8090".into()),
                ("-c".into(), "32768".into()),
                ("--flash-attn".into(), String::new()),
            ]
        );
    }

    #[test]
    fn spec_5_1_profile_appends_at_profile_position() {
        let cfg = parse(
            r#"llama-server "serve" {
                host "0.0.0.0"
                port 8090
                c 32768
                flash-attn #true
                qwen-coder {
                    m "/m1"
                    -ngl 999
                    -ts "0.5,0.5"
                }
            }"#,
        );
        let r = resolve(&cfg, "serve", Some("qwen-coder")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("--host".into(), "0.0.0.0".into()),
                ("--port".into(), "8090".into()),
                ("-c".into(), "32768".into()),
                ("--flash-attn".into(), String::new()),
                ("-m".into(), "/m1".into()),
                ("-ngl".into(), "999".into()),
                ("-ts".into(), "0.5,0.5".into()),
            ]
        );
    }

    #[test]
    fn spec_5_1_profile_overrides_default_at_default_position() {
        let cfg = parse(
            r#"llama-server "serve" {
                host "0.0.0.0"
                port 8090
                llama3 {
                    m "/m2"
                    port 8091
                }
            }"#,
        );
        let r = resolve(&cfg, "serve", Some("llama3")).unwrap();
        // `--port` keeps its default position (between host and the
        // profile's own contributions), but takes the profile's value.
        assert_eq!(
            flatten(&r),
            vec![
                ("--host".into(), "0.0.0.0".into()),
                ("--port".into(), "8091".into()),
                ("-m".into(), "/m2".into()),
            ]
        );
    }

    // --- §2.8.1 first-occurrence positioning ---

    #[test]
    fn first_occurrence_when_default_is_first() {
        let cfg = parse(
            r"some-tool {
                timeout 10
                verbose #true
                fast {
                    timeout 5
                }
            }",
        );
        let r = resolve(&cfg, "some-tool", Some("fast")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("--timeout".into(), "5".into()),
                ("--verbose".into(), String::new()),
            ]
        );
    }

    #[test]
    fn first_occurrence_when_profile_is_first() {
        let cfg = parse(
            r"some-tool {
                fast {
                    timeout 5
                }
                timeout 10
                verbose #true
            }",
        );
        let r = resolve(&cfg, "some-tool", Some("fast")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("--timeout".into(), "5".into()),
                ("--verbose".into(), String::new()),
            ]
        );
    }

    // --- §2.7 / §2.8 profiles as positional slots ---

    #[test]
    fn profile_slot_position_is_preserved() {
        let cfg = parse(
            r#"some-tool {
                "default-positional"
                profile-a {
                    timeout 30
                }
                profile-b {
                    timeout 60
                }
                verbose #true
                profile-c {
                    timeout 90
                }
            }"#,
        );
        let r = resolve(&cfg, "some-tool", Some("profile-c")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                (String::new(), "default-positional".into()),
                ("--verbose".into(), String::new()),
                ("--timeout".into(), "90".into()),
            ]
        );
    }

    // --- §2.8.2 cross-type override and suppression ---

    #[test]
    fn quiet_profile_suppresses_everything() {
        let cfg = parse(
            r#"some-tool {
                xxx "test"
                timeout 30
                verbose #true
                quiet {
                    xxx #false
                    timeout #false
                    verbose #false
                }
            }"#,
        );
        let r = resolve(&cfg, "some-tool", Some("quiet")).unwrap();
        assert_eq!(flatten(&r), Vec::<(String, String)>::new());
    }

    #[test]
    fn loud_profile_overrides_string_and_number() {
        let cfg = parse(
            r#"some-tool {
                xxx "test"
                timeout 30
                verbose #true
                loud {
                    xxx "verbose-mode"
                    timeout 5
                }
            }"#,
        );
        let r = resolve(&cfg, "some-tool", Some("loud")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("--xxx".into(), "verbose-mode".into()),
                ("--timeout".into(), "5".into()),
                ("--verbose".into(), String::new()),
            ]
        );
    }

    #[test]
    fn flag_form_converts_string_default_to_bare_flag() {
        let cfg = parse(
            r#"some-tool {
                xxx "test"
                timeout 30
                verbose #true
                flag-form { xxx #true }
            }"#,
        );
        let r = resolve(&cfg, "some-tool", Some("flag-form")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("--xxx".into(), String::new()),
                ("--timeout".into(), "30".into()),
                ("--verbose".into(), String::new()),
            ]
        );
    }

    #[test]
    fn no_log_suppresses_string_default_with_false() {
        let cfg = parse(
            r#"some-tool {
                verbose #true
                timeout 30
                log-file "/var/log/some-tool.log"
                no-log { log-file #false }
            }"#,
        );
        let r = resolve(&cfg, "some-tool", Some("no-log")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("--verbose".into(), String::new()),
                ("--timeout".into(), "30".into()),
            ]
        );
    }

    // --- §5.3.1 interleaved positionals ---

    #[test]
    fn ffmpeg_interleaved_positional() {
        let cfg = parse(
            r#"ffmpeg "transcode" {
                h264 {
                    i "input.mp4"
                    -c:v "libx264"
                    "output.mp4"
                }
            }"#,
        );
        let r = resolve(&cfg, "transcode", Some("h264")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("-i".into(), "input.mp4".into()),
                ("-c:v".into(), "libx264".into()),
                (String::new(), "output.mp4".into()),
            ]
        );
    }

    #[test]
    fn git_clone_interleaved() {
        let cfg = parse(
            r#"git {
                clone-myrepo {
                    "clone"
                    "https://github.com/me/repo.git"
                    depth 1
                }
            }"#,
        );
        let r = resolve(&cfg, "git", Some("clone-myrepo")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                (String::new(), "clone".into()),
                (String::new(), "https://github.com/me/repo.git".into()),
                ("--depth".into(), "1".into()),
            ]
        );
    }

    // --- §5.4 boolean vs string distinction ---

    #[test]
    fn enabled_string_vs_bare_flag() {
        let cfg = parse(
            r#"mytool {
                enabled-true {
                    enabled "true"
                }
                enabled-flag {
                    enabled #true
                }
            }"#,
        );
        let r1 = resolve(&cfg, "mytool", Some("enabled-true")).unwrap();
        assert_eq!(flatten(&r1), vec![("--enabled".into(), "true".into())]);
        let r2 = resolve(&cfg, "mytool", Some("enabled-flag")).unwrap();
        assert_eq!(flatten(&r2), vec![("--enabled".into(), String::new())]);
    }

    // --- did-you-mean nuance ---

    #[test]
    fn did_you_mean_off_by_two_is_caught() {
        let cfg = parse(r"qwen-coder {}");
        let err = resolve(&cfg, "qwen-codr", None).unwrap_err();
        let Error::UnknownCommand { help, .. } = err else {
            panic!();
        };
        assert!(help.contains("qwen-coder"));
    }

    #[test]
    fn did_you_mean_returns_none_for_completely_different_input() {
        let cfg = parse(r"qwen-coder {}");
        let err = resolve(&cfg, "totally-unrelated", None).unwrap_err();
        let Error::UnknownCommand { help, .. } = err else {
            panic!();
        };
        assert!(!help.contains("did you mean"));
    }

    #[test]
    fn flag_key_field_used() {
        // Sanity-check that resolve preserves FlagKey distinctions.
        let cfg = parse("foo {\n  -ngl 999\n  verbose #true\n}\n");
        let r = resolve(&cfg, "foo", None).unwrap();
        let Argument::Flag { key, .. } = &r.args[0] else {
            panic!();
        };
        assert!(matches!(key, FlagKey::Verbatim(s) if s == "-ngl"));
    }

    // --- §2.8 repeat-mode (multi-occurrence) merge ---

    #[test]
    fn defaults_only_repeated_keys_emit_in_order() {
        // gcc-style: two unmarked default occurrences resolve to
        // repeat mode (|D_unmarked| > 1).
        let cfg = parse(
            r#"gcc {
                I "/usr/include"
                I "/opt/include"
            }"#,
        );
        let r = resolve(&cfg, "gcc", None).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("-I".into(), "/usr/include".into()),
                ("-I".into(), "/opt/include".into()),
            ]
        );
    }

    #[test]
    fn profile_only_repeated_keys_emit_in_order() {
        let cfg = parse(
            r#"curl {
                with-headers {
                    header "X-A: 1"
                    header "X-B: 2"
                }
            }"#,
        );
        let r = resolve(&cfg, "curl", Some("with-headers")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("--header".into(), "X-A: 1".into()),
                ("--header".into(), "X-B: 2".into()),
            ]
        );
    }

    #[test]
    fn repeat_mode_when_default_has_two_and_profile_adds_one() {
        let cfg = parse(
            r#"gcc {
                I "/usr/include"
                I "/opt/include"
                project-a {
                    I "/proj/a/include"
                }
            }"#,
        );
        let r = resolve(&cfg, "gcc", Some("project-a")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("-I".into(), "/usr/include".into()),
                ("-I".into(), "/opt/include".into()),
                ("-I".into(), "/proj/a/include".into()),
            ]
        );
    }

    #[test]
    fn repeat_mode_when_default_has_one_and_profile_has_two() {
        // |D_unmarked|=1, |P_unmarked|=2 → repeat mode → all three
        // emit. The default does NOT get overridden in this shape.
        let cfg = parse(
            r#"gcc {
                I "/usr/include"
                add-two {
                    I "/a"
                    I "/b"
                }
            }"#,
        );
        let r = resolve(&cfg, "gcc", Some("add-two")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("-I".into(), "/usr/include".into()),
                ("-I".into(), "/a".into()),
                ("-I".into(), "/b".into()),
            ]
        );
    }

    #[test]
    fn count_flag_pattern_v_three_times() {
        // Three occurrences of `v #true` resolve to `-v -v -v`.
        let cfg = parse(
            r"some-tool {
                v #true
                v #true
                v #true
            }",
        );
        let r = resolve(&cfg, "some-tool", None).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("-v".into(), String::new()),
                ("-v".into(), String::new()),
                ("-v".into(), String::new()),
            ]
        );
    }

    // --- §2.8 markerless `#false` clear ---

    #[test]
    fn profile_false_clears_multi_default_list() {
        let cfg = parse(
            r#"gcc {
                I "/a"
                I "/b"
                bare {
                    I #false
                }
            }"#,
        );
        let r = resolve(&cfg, "gcc", Some("bare")).unwrap();
        assert_eq!(flatten(&r), Vec::<(String, String)>::new());
    }

    #[test]
    fn profile_false_then_value_clears_then_adds() {
        // The profile clears defaults' I list, then adds its own.
        let cfg = parse(
            r#"gcc {
                I "/a"
                I "/b"
                custom {
                    I #false
                    I "/mine"
                }
            }"#,
        );
        let r = resolve(&cfg, "gcc", Some("custom")).unwrap();
        // After clear+add, we're left with one profile occurrence —
        // single mode emits it at the profile's position.
        assert_eq!(flatten(&r), vec![("-I".into(), "/mine".into())]);
    }

    #[test]
    fn default_false_in_middle_of_repeats_drops_only_itself() {
        let cfg = parse(
            r#"gcc {
                I "/a"
                I #false
                I "/b"
            }"#,
        );
        let r = resolve(&cfg, "gcc", None).unwrap();
        assert_eq!(
            flatten(&r),
            vec![("-I".into(), "/a".into()), ("-I".into(), "/b".into()),]
        );
    }

    // --- §2.5 / §2.8 explicit append marker ---

    #[test]
    fn marked_profile_adds_to_single_default() {
        // The blind-spot case for the markerless rule: `+` lets the
        // profile add an occurrence without overriding the default.
        let cfg = parse(
            r#"gcc {
                I "/usr/include"
                proj-extras {
                    +I "/proj/include"
                }
            }"#,
        );
        let r = resolve(&cfg, "gcc", Some("proj-extras")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("-I".into(), "/usr/include".into()),
                ("-I".into(), "/proj/include".into()),
            ]
        );
    }

    #[test]
    fn unmarked_profile_overrides_single_default_in_v1_mode() {
        // Same shape as above but without the `+`: single+single
        // single-mode → v1 override at default's position.
        let cfg = parse(
            r#"gcc {
                I "/usr/include"
                proj-replace {
                    I "/proj/include"
                }
            }"#,
        );
        let r = resolve(&cfg, "gcc", Some("proj-replace")).unwrap();
        assert_eq!(flatten(&r), vec![("-I".into(), "/proj/include".into())]);
    }

    #[test]
    fn unmarked_overrides_default_marked_emits_separately() {
        // Profile has one unmarked + one marked. Unmarked single-mode
        // overrides the default at default's position; marked emits
        // at its own position.
        let cfg = parse(
            r#"gcc {
                I "/usr/include"
                mixed {
                    I "/replace"
                    +I "/extra"
                }
            }"#,
        );
        let r = resolve(&cfg, "gcc", Some("mixed")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("-I".into(), "/replace".into()),
                ("-I".into(), "/extra".into()),
            ]
        );
    }

    #[test]
    fn marked_default_emits_then_unmarked_single_mode() {
        // Default has a marked + an unmarked. Profile has one
        // unmarked. The marked default always emits; the unmarked
        // default + unmarked profile resolve in single mode.
        let cfg = parse(
            r#"gcc {
                +I "/always"
                I "/dflt"
                proj {
                    I "/proj"
                }
            }"#,
        );
        let r = resolve(&cfg, "gcc", Some("proj")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("-I".into(), "/always".into()),
                ("-I".into(), "/proj".into()),
            ]
        );
    }

    #[test]
    fn profile_false_clears_marked_default_too() {
        // Profile-side `#false` clears every default occurrence of
        // the key, regardless of marker.
        let cfg = parse(
            r#"gcc {
                +I "/always"
                I "/dflt"
                bare {
                    I #false
                }
            }"#,
        );
        let r = resolve(&cfg, "gcc", Some("bare")).unwrap();
        assert_eq!(flatten(&r), Vec::<(String, String)>::new());
    }

    #[test]
    fn marked_profile_with_no_default() {
        // `+` on a profile flag with no matching default is harmless:
        // it's the only entry, emits at its own position.
        let cfg = parse(
            r#"foo {
                proj {
                    +I "/proj"
                }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("proj")).unwrap();
        assert_eq!(flatten(&r), vec![("-I".into(), "/proj".into())]);
    }

    #[test]
    fn two_marked_entries_in_one_profile() {
        // Two `+`-marked entries for the same key in one profile —
        // both should emit at their own positions, since marker
        // skips collapse.
        let cfg = parse(
            r#"foo {
                proj {
                    +I "/a"
                    +I "/b"
                }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("proj")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![("-I".into(), "/a".into()), ("-I".into(), "/b".into()),]
        );
    }

    #[test]
    fn marked_default_with_no_profile_selected() {
        // `+` in defaults with no profile selected is harmless: only
        // entry, emits at its own position.
        let cfg = parse(r#"foo { +I "/dflt" }"#);
        let r = resolve(&cfg, "foo", None).unwrap();
        assert_eq!(flatten(&r), vec![("-I".into(), "/dflt".into())]);
    }

    #[test]
    fn marked_boolean_flag_emits() {
        // `+v #true` is a marked boolean: marker forces own-position
        // emit, value `#true` emits as bare flag (no value).
        let cfg = parse(
            r"foo {
                v #true
                more {
                    +v #true
                }
            }",
        );
        let r = resolve(&cfg, "foo", Some("more")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![("-v".into(), String::new()), ("-v".into(), String::new()),]
        );
    }

    #[test]
    fn resolved_form_collision_keys_marked_and_unmarked() {
        // `+host` and unmarked `--host` both resolve to `--host`,
        // so they share the same per-key plan. Marked emits at its
        // own position; unmarked single-mode emits at its position
        // with its own value (no profile to override with).
        let cfg = parse(
            r#"foo {
                +host "a"
                --host "b"
            }"#,
        );
        let r = resolve(&cfg, "foo", None).unwrap();
        assert_eq!(
            flatten(&r),
            vec![("--host".into(), "a".into()), ("--host".into(), "b".into()),]
        );
    }

    // --- §2.10 / §2.11 env-var resolution ---

    #[test]
    fn env_defaults_only_no_profile() {
        let cfg = parse(
            r#"foo {
                (env)A "1"
                (env)B "2"
            }"#,
        );
        let r = resolve(&cfg, "foo", None).unwrap();
        assert_eq!(
            r.env,
            vec![
                EnvOp::Set {
                    name: "A".into(),
                    value: "1".into()
                },
                EnvOp::Set {
                    name: "B".into(),
                    value: "2".into()
                },
            ]
        );
    }

    #[test]
    fn env_profile_only_when_no_defaults() {
        let cfg = parse(
            r#"foo {
                fast {
                    (env)X "y"
                }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("fast")).unwrap();
        assert_eq!(
            r.env,
            vec![EnvOp::Set {
                name: "X".into(),
                value: "y".into()
            }]
        );
    }

    #[test]
    fn env_profile_overrides_default() {
        let cfg = parse(
            r#"foo {
                (env)A "default"
                fast {
                    (env)A "override"
                }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("fast")).unwrap();
        assert_eq!(
            r.env,
            vec![EnvOp::Set {
                name: "A".into(),
                value: "override".into()
            }]
        );
    }

    #[test]
    fn env_profile_unset_clears_default() {
        let cfg = parse(
            r#"foo {
                (env)A "default"
                clear {
                    (env)A #false
                }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("clear")).unwrap();
        assert_eq!(r.env, vec![EnvOp::Unset { name: "A".into() }]);
    }

    #[test]
    fn env_default_unset_passes_through_when_no_profile_override() {
        let cfg = parse(
            r"foo {
                (env)PATH #false
            }",
        );
        let r = resolve(&cfg, "foo", None).unwrap();
        assert_eq!(
            r.env,
            vec![EnvOp::Unset {
                name: "PATH".into()
            }]
        );
    }

    #[test]
    fn env_profile_set_overrides_default_unset() {
        // Per §2.11 precedence: profile-side `Set` wins over
        // default-side `Unset`. The default would otherwise call
        // env_remove; the profile re-introduces the variable.
        let cfg = parse(
            r#"foo {
                (env)A #false
                fast {
                    (env)A "from-profile"
                }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("fast")).unwrap();
        assert_eq!(
            r.env,
            vec![EnvOp::Set {
                name: "A".into(),
                value: "from-profile".into()
            }]
        );
    }

    #[test]
    fn env_empty_string_value_round_trips() {
        // `""` is a meaningful POSIX env value (set, but empty),
        // distinct from `#false` (unset). Pin that we don't drop it.
        let cfg = parse(r#"foo { (env)A "" }"#);
        let r = resolve(&cfg, "foo", None).unwrap();
        assert_eq!(
            r.env,
            vec![EnvOp::Set {
                name: "A".into(),
                value: String::new()
            }]
        );
    }

    #[test]
    fn env_first_occurrence_ordering_default_first() {
        // Defaults define A then B; profile overrides B and adds C.
        // Expected order: A, B, C — defaults' walk order then profile
        // additions.
        let cfg = parse(
            r#"foo {
                (env)A "1"
                (env)B "2"
                fast {
                    (env)B "overridden"
                    (env)C "3"
                }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("fast")).unwrap();
        assert_eq!(
            r.env,
            vec![
                EnvOp::Set {
                    name: "A".into(),
                    value: "1".into()
                },
                EnvOp::Set {
                    name: "B".into(),
                    value: "overridden".into()
                },
                EnvOp::Set {
                    name: "C".into(),
                    value: "3".into()
                },
            ]
        );
    }

    #[test]
    fn env_does_not_appear_in_args() {
        // §2.10: env decls do not appear on the resolved argv.
        let cfg = parse(
            r#"foo {
                host "0.0.0.0"
                (env)OLLAMA_HOST "1"
            }"#,
        );
        let r = resolve(&cfg, "foo", None).unwrap();
        assert_eq!(flatten(&r), vec![("--host".into(), "0.0.0.0".into())]);
        assert_eq!(r.env.len(), 1);
    }

    #[test]
    fn count_flag_clear_then_replace() {
        // Profile clears the default count and sets a different one.
        // Profile `#false` wipes defaults; the two surviving
        // unmarked profile entries trigger repeat mode.
        let cfg = parse(
            r"foo {
                v #true
                v #true
                v #true
                medium {
                    v #false
                    v #true
                    v #true
                }
            }",
        );
        let r = resolve(&cfg, "foo", Some("medium")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![("-v".into(), String::new()), ("-v".into(), String::new()),]
        );
    }
}
