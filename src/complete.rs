//! Emit completion candidates for shell completion scripts.
//!
//! Two emitters back the hand-rolled scripts in
//! [`crate::completions`]: [`print_commands`] for the first
//! positional and [`print_profiles`] for the second. Both write one
//! candidate per line to stdout and never produce stderr —
//! completion scripts must never fail mid-tab.
//!
//! Eligibility rules mirror the lookup rules used by
//! [`crate::resolve`] (`SPEC.md` §2.9 / §4):
//!
//! - A bare command name is a candidate only if it appears exactly
//!   once. Duplicated names are excluded because typing one produces
//!   an `AmbiguousCommand` error.
//! - Every alias is a candidate (alias uniqueness is enforced at
//!   validation time).
//! - For [`print_profiles`], the resolution rule matches §4 step 3
//!   except an unknown or duplicated bare name produces empty
//!   output rather than an error.

use std::collections::HashMap;

use crate::config::{CommandChild, Config};
use crate::resolve;

/// Print every name that is a valid lookup key plus every alias,
/// one per line. Iteration is in source order; per-command output
/// is `name` (when eligible) followed by `alias` (when present).
pub fn print_commands(config: &Config) {
    let mut name_counts: HashMap<&str, usize> = HashMap::new();
    for cmd in &config.commands {
        *name_counts.entry(cmd.name.as_str()).or_insert(0) += 1;
    }
    for cmd in &config.commands {
        if name_counts[cmd.name.as_str()] == 1 {
            println!("{}", cmd.name);
        }
        if let Some(alias) = &cmd.alias {
            println!("{alias}");
        }
    }
}

/// Print profile names attached to the command identified by
/// `name` (a command name or alias), one per line, in source
/// order. Unknown or duplicated-bare-name inputs print nothing.
pub fn print_profiles(config: &Config, name: &str) {
    let Some(cmd) = resolve::find_for_completion(config, name) else {
        return;
    };
    for child in &cmd.children {
        if let CommandChild::Profile { name, .. } = child {
            println!("{name}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse::parse_str;

    fn parse(input: &str) -> Config {
        parse_str(input, "test.kdl").expect("invariant: test KDL must parse")
    }

    /// Capture `print_commands`'s stdout via a child invocation of
    /// the closure. We can't redirect global stdout cleanly inside
    /// a unit test without unsafe, so we exercise the same logic
    /// via a small in-test re-implementation that accumulates into
    /// a `Vec<String>`. The shape mirrors `print_commands` exactly;
    /// the integration tests cover end-to-end stdout behavior.
    fn collect_commands(config: &Config) -> Vec<String> {
        let mut name_counts: HashMap<&str, usize> = HashMap::new();
        for cmd in &config.commands {
            *name_counts.entry(cmd.name.as_str()).or_insert(0) += 1;
        }
        let mut out = Vec::new();
        for cmd in &config.commands {
            if name_counts[cmd.name.as_str()] == 1 {
                out.push(cmd.name.clone());
            }
            if let Some(alias) = &cmd.alias {
                out.push(alias.clone());
            }
        }
        out
    }

    fn collect_profiles(config: &Config, name: &str) -> Vec<String> {
        let Some(cmd) = resolve::find_for_completion(config, name) else {
            return Vec::new();
        };
        cmd.children
            .iter()
            .filter_map(|c| match c {
                CommandChild::Profile { name, .. } => Some(name.clone()),
                CommandChild::Default(_) => None,
            })
            .collect()
    }

    #[test]
    fn unique_name_emits_both_name_and_alias() {
        let cfg = parse(r#"llama-server "serve" {}"#);
        assert_eq!(collect_commands(&cfg), vec!["llama-server", "serve"]);
    }

    #[test]
    fn unique_name_without_alias_emits_just_name() {
        let cfg = parse(r"foo {}");
        assert_eq!(collect_commands(&cfg), vec!["foo"]);
    }

    #[test]
    fn duplicated_name_is_excluded_aliases_kept() {
        let cfg = parse(
            r#"llama-server "serve1" {}
            llama-server "serve2" {}"#,
        );
        assert_eq!(collect_commands(&cfg), vec!["serve1", "serve2"]);
    }

    #[test]
    fn mixed_unique_and_duplicated_names() {
        let cfg = parse(
            r#"foo "f" {}
            llama-server "serve1" {}
            llama-server "serve2" {}
            bar {}"#,
        );
        assert_eq!(
            collect_commands(&cfg),
            vec!["foo", "f", "serve1", "serve2", "bar"]
        );
    }

    #[test]
    fn profiles_via_command_name() {
        let cfg = parse(
            r"foo {
                fast {}
                slow {}
            }",
        );
        assert_eq!(collect_profiles(&cfg, "foo"), vec!["fast", "slow"]);
    }

    #[test]
    fn profiles_via_alias() {
        let cfg = parse(
            r#"llama-server "serve" {
                qwen-coder {}
                llama3 {}
            }"#,
        );
        assert_eq!(
            collect_profiles(&cfg, "serve"),
            vec!["qwen-coder", "llama3"]
        );
    }

    #[test]
    fn profiles_for_duplicated_name_are_empty() {
        let cfg = parse(
            r#"llama-server "a" { p1 {} }
            llama-server "b" { p2 {} }"#,
        );
        assert!(collect_profiles(&cfg, "llama-server").is_empty());
    }

    #[test]
    fn profiles_for_alias_of_duplicated_name_only_returns_that_occurrence() {
        let cfg = parse(
            r#"llama-server "a" { p1 {} }
            llama-server "b" { p2 {} }"#,
        );
        assert_eq!(collect_profiles(&cfg, "a"), vec!["p1"]);
        assert_eq!(collect_profiles(&cfg, "b"), vec!["p2"]);
    }

    #[test]
    fn profiles_for_unknown_name_are_empty() {
        let cfg = parse(r"foo { fast {} }");
        assert!(collect_profiles(&cfg, "nope").is_empty());
    }

    #[test]
    fn profiles_for_command_without_profiles_are_empty() {
        let cfg = parse(r#"foo { host "x" }"#);
        assert!(collect_profiles(&cfg, "foo").is_empty());
    }
}
