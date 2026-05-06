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
//! 4. First-occurrence collapse (`SPEC.md` §2.8.1): for each flag
//!    key (compared in resolved CLI form per §2.5) keep the
//!    candidate at the first walk position. The effective value is
//!    the profile's contribution if any, else the default's.
//! 5. Suppress flags whose effective value is `#false` (`SPEC.md`
//!    §2.8.2).
//!
//! Positionals are not subject to override or suppression and are
//! emitted at the position they were walked, in source order.

use std::collections::HashMap;

use crate::config::{Argument, Command, CommandChild, Config, FlagValue};
use crate::errors::{Error, Result};

/// Output of a successful resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The program to execute (the command's `name` field).
    pub program: String,
    /// The candidate list after walk + collapse + suppression.
    pub args: Vec<Argument>,
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

    // Step 3: walk children, marking each candidate's source.
    let mut candidates: Vec<(Argument, bool)> = Vec::new();
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

    // Step 4: first-occurrence collapse.
    let mut first_index: HashMap<String, usize> = HashMap::new();
    let mut effective: HashMap<String, FlagValue> = HashMap::new();
    for (i, (arg, from_profile)) in candidates.iter().enumerate() {
        if let Argument::Flag { key, value, .. } = arg {
            let resolved_key = key.to_cli_flag();
            first_index.entry(resolved_key.clone()).or_insert(i);
            // Profile contributions overwrite; default contributions
            // only insert if no value yet (validate already ensures
            // each side contributes at most once per scope).
            if *from_profile {
                effective.insert(resolved_key, value.clone());
            } else {
                effective
                    .entry(resolved_key)
                    .or_insert_with(|| value.clone());
            }
        }
    }

    // Step 5: build the final list. Positionals always emit; flags
    // emit only at their first-occurrence index, with the effective
    // value, and `#false` suppresses entirely.
    let mut out: Vec<Argument> = Vec::with_capacity(candidates.len());
    for (i, (arg, _)) in candidates.into_iter().enumerate() {
        match arg {
            Argument::Positional(_) => out.push(arg),
            Argument::Flag {
                key,
                key_span,
                value: _,
            } => {
                let resolved_key = key.to_cli_flag();
                if first_index.get(&resolved_key) == Some(&i) {
                    let value = effective
                        .remove(&resolved_key)
                        .expect("invariant: effective is keyed by every flag's resolved key");
                    if !matches!(value, FlagValue::Bool(false)) {
                        out.push(Argument::Flag {
                            key,
                            key_span,
                            value,
                        });
                    }
                }
            }
        }
    }

    Ok(Resolved {
        program: cmd.name.clone(),
        args: out,
    })
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
}
