//! Enforce `SPEC.md` §2.9 constraints on a parsed [`Config`].
//!
//! The parser handles structural correctness (`SPEC.md` §2.2-§2.6).
//! This module handles the semantic constraints listed in §2.9:
//!
//! - Names (commands, aliases, profiles) must not start with `-`
//!   (would conflict with `jig`'s own flags) or `+` (reserved for
//!   the explicit append marker on flag keys, §2.5).
//! - A command name may appear more than once, but only when every
//!   occurrence declares an alias (so each entry is reachable).
//! - Aliases must be unique across the file.
//! - An alias may not collide with any non-duplicated command name
//!   (a command may declare an alias equal to its own name when
//!   that name is unique). A duplicated command name may not be
//!   used as an alias anywhere.
//! - Profile names must be unique within a command.
//!
//! Repeated flag keys within a scope are *allowed*; the resolver
//! distinguishes single-mode vs repeat-mode merge per `SPEC.md` §2.8
//! based on multiplicity and the `+` append marker.
//!
//! On the first violation found we return an [`Error`] carrying the
//! relevant source spans for two-span diagnostics per §7.4.

use std::collections::HashMap;

use miette::{NamedSource, SourceSpan};

use super::{CommandChild, Config};
use crate::errors::{Error, Result};

/// Validate `config` against `SPEC.md` §2.9. `src` is attached to
/// any returned diagnostic so spans render against the right file.
///
/// # Errors
///
/// Returns the first §2.9 violation encountered. See module docs
/// for the constraint list.
pub fn validate(config: &Config, src: &NamedSource<String>) -> Result<()> {
    check_reserved_name_prefixes(config, src)?;
    check_command_and_alias_names(config, src)?;
    check_profiles_within_each_command(config, src)?;
    Ok(())
}

fn check_reserved_name_prefixes(config: &Config, src: &NamedSource<String>) -> Result<()> {
    for cmd in &config.commands {
        check_name_prefix("command", &cmd.name, cmd.name_span, src)?;
        if let (Some(alias), Some(span)) = (&cmd.alias, cmd.alias_span) {
            check_name_prefix("alias", alias, span, src)?;
        }
        for child in &cmd.children {
            if let CommandChild::Profile {
                name, name_span, ..
            } = child
            {
                check_name_prefix("profile", name, *name_span, src)?;
            }
        }
    }
    Ok(())
}

fn check_name_prefix(
    kind: &'static str,
    name: &str,
    span: SourceSpan,
    src: &NamedSource<String>,
) -> Result<()> {
    if name.starts_with('-') {
        return Err(Error::LeadingDashName {
            kind,
            name: name.to_string(),
            src: src.clone(),
            span,
        });
    }
    if name.starts_with('+') {
        return Err(Error::LeadingPlusName {
            kind,
            name: name.to_string(),
            src: src.clone(),
            span,
        });
    }
    Ok(())
}

fn check_command_and_alias_names(config: &Config, src: &NamedSource<String>) -> Result<()> {
    // Pass A: collect all occurrences of each command name. A name
    // may now appear more than once, so we keep every span — the
    // duplicate-without-alias check below needs them, and so does
    // the alias-vs-name collision check.
    let mut name_occurrences: HashMap<&str, Vec<(SourceSpan, Option<SourceSpan>)>> = HashMap::new();
    for cmd in &config.commands {
        name_occurrences
            .entry(cmd.name.as_str())
            .or_default()
            .push((cmd.name_span, cmd.alias_span));
    }

    // Pass A.1: every occurrence of a duplicated name must have an
    // alias — otherwise the entry is unreachable. Report the spans
    // in source order so the rendered diagnostic reads naturally.
    for cmd in &config.commands {
        let occurrences = name_occurrences
            .get(cmd.name.as_str())
            .expect("invariant: name_occurrences is keyed by every command's name");
        if occurrences.len() > 1 && cmd.alias.is_none() {
            let other = occurrences
                .iter()
                .map(|(span, _)| *span)
                .find(|s| *s != cmd.name_span)
                .expect("invariant: occurrences.len() > 1 guarantees a different span exists");
            let (first, second) = if other.offset() < cmd.name_span.offset() {
                (other, cmd.name_span)
            } else {
                (cmd.name_span, other)
            };
            return Err(Error::DuplicateCommandWithoutAlias {
                name: cmd.name.clone(),
                src: src.clone(),
                first,
                second,
            });
        }
    }

    // Pass B: alias uniqueness across the file.
    let mut seen_aliases: HashMap<&str, SourceSpan> = HashMap::new();
    for cmd in &config.commands {
        if let (Some(alias), Some(span)) = (&cmd.alias, cmd.alias_span) {
            if let Some(&first) = seen_aliases.get(alias.as_str()) {
                return Err(Error::DuplicateAlias {
                    alias: alias.clone(),
                    src: src.clone(),
                    first,
                    second: span,
                });
            }
            seen_aliases.insert(alias, span);
        }
    }

    // Pass C: alias-vs-command-name collision.
    //
    //   - If the alias matches no command name, no collision.
    //   - If the alias matches a duplicated command name (>1 entry),
    //     reject — using the alias would shadow the bare-name
    //     ambiguity.
    //   - If the alias matches a unique command name and that name
    //     belongs to the same command, it's a self-alias and is
    //     allowed (harmless redundancy).
    //   - If the alias matches a unique command name belonging to a
    //     different command, reject.
    for cmd in &config.commands {
        if let (Some(alias), Some(alias_span)) = (&cmd.alias, cmd.alias_span) {
            let Some(occurrences) = name_occurrences.get(alias.as_str()) else {
                continue;
            };
            if occurrences.len() > 1 {
                // Prefer pointing at a different occurrence than
                // `cmd` itself, so the diagnostic exposes the
                // duplication. Matters for self-aliasing on a
                // duplicate, where `cmd`'s name span sits next to
                // the alias literal on the same line.
                let command_span = occurrences
                    .iter()
                    .map(|(span, _)| *span)
                    .find(|s| *s != cmd.name_span)
                    .unwrap_or(occurrences[0].0);
                return Err(Error::CommandAliasCollision {
                    name: alias.clone(),
                    src: src.clone(),
                    alias_span,
                    command_span,
                });
            }
            // Exactly one occurrence: self-alias if it's this command.
            let (only_name_span, _) = occurrences[0];
            if only_name_span == cmd.name_span {
                continue;
            }
            return Err(Error::CommandAliasCollision {
                name: alias.clone(),
                src: src.clone(),
                alias_span,
                command_span: only_name_span,
            });
        }
    }

    Ok(())
}

fn check_profiles_within_each_command(config: &Config, src: &NamedSource<String>) -> Result<()> {
    for cmd in &config.commands {
        let mut seen: HashMap<&str, SourceSpan> = HashMap::new();
        for child in &cmd.children {
            if let CommandChild::Profile {
                name, name_span, ..
            } = child
            {
                if let Some(&first) = seen.get(name.as_str()) {
                    return Err(Error::DuplicateProfile {
                        name: name.clone(),
                        command: cmd.name.clone(),
                        src: src.clone(),
                        first,
                        second: *name_span,
                    });
                }
                seen.insert(name, *name_span);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse::parse_str;

    fn parse_and_validate(input: &str) -> Result<()> {
        let cfg = parse_str(input, "test.kdl")?;
        let src = NamedSource::new("test.kdl", input.to_string());
        validate(&cfg, &src)
    }

    #[test]
    fn valid_config_passes() {
        parse_and_validate(
            r#"llama-server "serve" {
                host "0.0.0.0"
                port 8090
                qwen-coder { m "/p" }
                llama3 { m "/q" }
            }
            gemma-server { host "0.0.0.0" }
            "#,
        )
        .unwrap();
    }

    #[test]
    fn self_alias_is_allowed() {
        // A command whose alias equals its own name is fine.
        parse_and_validate(r#"foo "foo" {}"#).unwrap();
    }

    // --- §2.9 leading-dash rule ---

    #[test]
    fn leading_dash_command_name_rejected() {
        let err = parse_and_validate(r#""-bad" {}"#).unwrap_err();
        assert!(matches!(
            err,
            Error::LeadingDashName {
                kind: "command",
                ..
            }
        ));
    }

    #[test]
    fn leading_dash_alias_rejected() {
        let err = parse_and_validate(r#"foo "-bad" {}"#).unwrap_err();
        assert!(matches!(err, Error::LeadingDashName { kind: "alias", .. }));
    }

    #[test]
    fn leading_dash_profile_name_rejected() {
        let err = parse_and_validate(
            r#"foo {
                "-bad" {}
            }"#,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            Error::LeadingDashName {
                kind: "profile",
                ..
            }
        ));
    }

    // --- §2.9 command-name uniqueness ---

    #[test]
    fn duplicate_command_name_with_distinct_aliases_is_allowed() {
        // The relaxed rule: same binary, two profile sets, distinct
        // aliases — both reachable via their aliases.
        parse_and_validate(
            r#"llama-server "serve1" {
                a #true
            }
            llama-server "serve2" {
                b #true
            }"#,
        )
        .unwrap();
    }

    #[test]
    fn duplicate_command_name_without_alias_rejected() {
        let err = parse_and_validate(
            r"foo {}
            foo {}",
        )
        .unwrap_err();
        assert!(matches!(err, Error::DuplicateCommandWithoutAlias { .. }));
    }

    #[test]
    fn duplicate_command_one_alias_missing_rejected() {
        let err = parse_and_validate(
            r#"foo "x" {}
            foo {}"#,
        )
        .unwrap_err();
        assert!(matches!(err, Error::DuplicateCommandWithoutAlias { .. }));
    }

    // --- §2.9 alias uniqueness ---

    #[test]
    fn duplicate_alias_rejected() {
        let err = parse_and_validate(
            r#"llama-server "serve" {}
            gemma-server "serve" {}"#,
        )
        .unwrap_err();
        assert!(matches!(err, Error::DuplicateAlias { .. }));
    }

    // --- §2.9 cross-command alias/command collision ---

    #[test]
    fn alias_colliding_with_other_command_name_rejected() {
        let err = parse_and_validate(
            r#"serve {}
            llama-server "serve" {}"#,
        )
        .unwrap_err();
        assert!(matches!(err, Error::CommandAliasCollision { .. }));
    }

    #[test]
    fn alias_equal_to_duplicated_command_name_rejected() {
        // `foo` appears twice (with distinct aliases), so no other
        // command may use `foo` as its alias.
        let err = parse_and_validate(
            r#"foo "x" {}
            foo "y" {}
            cool "foo" {}"#,
        )
        .unwrap_err();
        assert!(matches!(err, Error::CommandAliasCollision { .. }));
    }

    #[test]
    fn three_duplicates_without_alias_rejected() {
        let err = parse_and_validate(
            r"foo {}
            foo {}
            foo {}",
        )
        .unwrap_err();
        assert!(matches!(err, Error::DuplicateCommandWithoutAlias { .. }));
    }

    #[test]
    fn duplicate_names_plus_duplicate_aliases_reports_no_alias_first() {
        // `foo` is duplicated and a third occurrence reuses an
        // existing alias. Pass A.1 fires before Pass B, so the
        // missing-alias case is reported (the duplicated alias would
        // be reported next if this were fixed). This pins the order.
        let err = parse_and_validate(
            r#"foo "x" {}
            foo {}
            foo "x" {}"#,
        )
        .unwrap_err();
        assert!(matches!(err, Error::DuplicateCommandWithoutAlias { .. }));
    }

    #[test]
    fn duplicate_aliases_without_missing_alias_still_caught() {
        // All occurrences have aliases (so Pass A.1 doesn't fire)
        // but two share an alias — Pass B reports it.
        let err = parse_and_validate(
            r#"foo "x" {}
            foo "x" {}"#,
        )
        .unwrap_err();
        assert!(matches!(err, Error::DuplicateAlias { .. }));
    }

    #[test]
    fn self_alias_on_duplicated_name_rejected() {
        // Self-aliasing is allowed only when the name is unique.
        let err = parse_and_validate(
            r#"foo "foo" {}
            foo "bar" {}"#,
        )
        .unwrap_err();
        assert!(matches!(err, Error::CommandAliasCollision { .. }));
    }

    // --- §2.9 profile-name uniqueness within command ---

    #[test]
    fn duplicate_profile_within_command_rejected() {
        let err = parse_and_validate(
            r"foo {
                fast {}
                fast {}
            }",
        )
        .unwrap_err();
        assert!(matches!(err, Error::DuplicateProfile { .. }));
    }

    #[test]
    fn same_profile_name_in_different_commands_is_fine() {
        parse_and_validate(
            r"foo {
                fast {}
            }
            bar {
                fast {}
            }",
        )
        .unwrap();
    }

    // --- §2.9: flag keys may now repeat within a scope (resolver
    //     handles single-mode vs repeat-mode merge per §2.8). ---

    #[test]
    fn duplicate_flag_key_in_defaults_is_fine() {
        // Was a parse error in v0.2; now allowed (gcc-style repeats).
        parse_and_validate(
            r#"gcc {
                I "/usr/include"
                I "/opt/include"
            }"#,
        )
        .unwrap();
    }

    #[test]
    fn duplicate_flag_key_in_profile_is_fine() {
        parse_and_validate(
            r"foo {
                fast {
                    timeout 5
                    timeout 10
                }
            }",
        )
        .unwrap();
    }

    #[test]
    fn flag_key_collision_across_scopes_is_fine() {
        // Defaults and a profile body have always been allowed to
        // share a key; this is what makes profile overrides work.
        parse_and_validate(
            r"foo {
                timeout 30
                fast {
                    timeout 5
                }
            }",
        )
        .unwrap();
    }

    #[test]
    fn flag_key_collision_across_two_profiles_is_fine() {
        parse_and_validate(
            r"foo {
                fast { timeout 5 }
                slow { timeout 60 }
            }",
        )
        .unwrap();
    }

    #[test]
    fn positionals_can_repeat_freely() {
        parse_and_validate(
            r#"foo {
                profile {
                    "input.mp4"
                    "input.mp4"
                    "output.mp4"
                }
            }"#,
        )
        .unwrap();
    }

    // --- §2.5: leading `+` reserved on names ---

    #[test]
    fn leading_plus_command_name_rejected() {
        let err = parse_and_validate(r#""+bad" {}"#).unwrap_err();
        assert!(matches!(
            err,
            Error::LeadingPlusName {
                kind: "command",
                ..
            }
        ));
    }

    #[test]
    fn leading_plus_alias_rejected() {
        let err = parse_and_validate(r#"foo "+bad" {}"#).unwrap_err();
        assert!(matches!(err, Error::LeadingPlusName { kind: "alias", .. }));
    }

    #[test]
    fn leading_plus_profile_name_rejected() {
        let err = parse_and_validate(
            r#"foo {
                "+bad" {}
            }"#,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            Error::LeadingPlusName {
                kind: "profile",
                ..
            }
        ));
    }
}
