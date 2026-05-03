//! Enforce `SPEC.md` §2.9 constraints on a parsed [`Config`].
//!
//! The parser handles structural correctness (`SPEC.md` §2.2-§2.6).
//! This module handles the semantic constraints listed in §2.9:
//!
//! - Names (commands, aliases, profiles) must not start with `-`.
//! - Command names must be unique across the file.
//! - Aliases must be unique across the file.
//! - A command name and an alias on a *different* command may not
//!   collide (a command may declare an alias equal to its own name).
//! - Profile names must be unique within a command.
//! - Each flag key, in its resolved CLI form (post-§2.5), must
//!   appear at most once within a single scope (a command's
//!   defaults or one profile body).
//!
//! On the first violation found we return an [`Error`] carrying the
//! relevant source spans for two-span diagnostics per §7.4.

use std::collections::HashMap;

use miette::{NamedSource, SourceSpan};

use super::{Argument, CommandChild, Config};
use crate::errors::{Error, Result};

/// Validate `config` against `SPEC.md` §2.9. `src` is attached to
/// any returned diagnostic so spans render against the right file.
///
/// # Errors
///
/// Returns the first §2.9 violation encountered. See module docs
/// for the constraint list.
pub fn validate(config: &Config, src: &NamedSource<String>) -> Result<()> {
    check_no_leading_dash(config, src)?;
    check_command_and_alias_names(config, src)?;
    check_profiles_within_each_command(config, src)?;
    check_flag_keys_within_each_scope(config, src)?;
    Ok(())
}

fn check_no_leading_dash(config: &Config, src: &NamedSource<String>) -> Result<()> {
    for cmd in &config.commands {
        if cmd.name.starts_with('-') {
            return Err(Error::LeadingDashName {
                kind: "command",
                name: cmd.name.clone(),
                src: src.clone(),
                span: cmd.name_span,
            });
        }
        if let (Some(alias), Some(span)) = (&cmd.alias, cmd.alias_span)
            && alias.starts_with('-')
        {
            return Err(Error::LeadingDashName {
                kind: "alias",
                name: alias.clone(),
                src: src.clone(),
                span,
            });
        }
        for child in &cmd.children {
            if let CommandChild::Profile {
                name, name_span, ..
            } = child
                && name.starts_with('-')
            {
                return Err(Error::LeadingDashName {
                    kind: "profile",
                    name: name.clone(),
                    src: src.clone(),
                    span: *name_span,
                });
            }
        }
    }
    Ok(())
}

fn check_command_and_alias_names(config: &Config, src: &NamedSource<String>) -> Result<()> {
    // Pass 1: command-name uniqueness.
    let mut seen_commands: HashMap<&str, SourceSpan> = HashMap::new();
    for cmd in &config.commands {
        if let Some(&first) = seen_commands.get(cmd.name.as_str()) {
            return Err(Error::DuplicateCommand {
                name: cmd.name.clone(),
                src: src.clone(),
                first,
                second: cmd.name_span,
            });
        }
        seen_commands.insert(&cmd.name, cmd.name_span);
    }

    // Pass 2: alias uniqueness.
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

    // Pass 3: cross-command alias-vs-command-name collision. A
    // command's alias matching its own command name is allowed
    // (harmless redundancy — see SPEC.md §2.9).
    for cmd in &config.commands {
        if let (Some(alias), Some(alias_span)) = (&cmd.alias, cmd.alias_span) {
            if alias == &cmd.name {
                continue;
            }
            if let Some(&command_span) = seen_commands.get(alias.as_str()) {
                return Err(Error::CommandAliasCollision {
                    name: alias.clone(),
                    src: src.clone(),
                    alias_span,
                    command_span,
                });
            }
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

fn check_flag_keys_within_each_scope(config: &Config, src: &NamedSource<String>) -> Result<()> {
    for cmd in &config.commands {
        // Defaults scope: every Default(Argument::Flag { .. }) child.
        let defaults_iter = cmd.children.iter().filter_map(|c| match c {
            CommandChild::Default(arg) => Some(arg),
            CommandChild::Profile { .. } => None,
        });
        check_scope(defaults_iter, src)?;

        // Each profile body is its own scope.
        for child in &cmd.children {
            if let CommandChild::Profile { args, .. } = child {
                check_scope(args.iter(), src)?;
            }
        }
    }
    Ok(())
}

fn check_scope<'a, I>(args: I, src: &NamedSource<String>) -> Result<()>
where
    I: IntoIterator<Item = &'a Argument>,
{
    let mut seen: HashMap<String, SourceSpan> = HashMap::new();
    for arg in args {
        if let Argument::Flag { key, key_span, .. } = arg {
            let resolved = key.to_cli_flag();
            if let Some(&first) = seen.get(&resolved) {
                return Err(Error::DuplicateFlagKey {
                    flag: resolved,
                    src: src.clone(),
                    first,
                    second: *key_span,
                });
            }
            seen.insert(resolved, *key_span);
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
    fn duplicate_command_name_rejected() {
        let err = parse_and_validate(
            r"foo {}
            foo {}",
        )
        .unwrap_err();
        assert!(matches!(err, Error::DuplicateCommand { .. }));
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

    // --- §2.9 flag-key uniqueness within scope ---

    #[test]
    fn duplicate_flag_key_in_defaults_rejected() {
        let err = parse_and_validate(
            r#"foo {
                host "a"
                host "b"
            }"#,
        )
        .unwrap_err();
        assert!(matches!(err, Error::DuplicateFlagKey { .. }));
    }

    #[test]
    fn duplicate_flag_key_in_profile_rejected() {
        let err = parse_and_validate(
            r"foo {
                fast {
                    timeout 5
                    timeout 10
                }
            }",
        )
        .unwrap_err();
        assert!(matches!(err, Error::DuplicateFlagKey { .. }));
    }

    #[test]
    fn resolved_form_collision_rejected() {
        // `host` and `--host` both resolve to `--host`.
        let err = parse_and_validate(
            r#"foo {
                host "a"
                --host "b"
            }"#,
        )
        .unwrap_err();
        let Error::DuplicateFlagKey { flag, .. } = &err else {
            panic!("expected DuplicateFlagKey, got {err:?}");
        };
        assert_eq!(flag, "--host");
    }

    #[test]
    fn short_form_collision_rejected() {
        // `m` resolves to `-m`, same as `-m`.
        let err = parse_and_validate(
            r#"foo {
                m "/p"
                -m "/q"
            }"#,
        )
        .unwrap_err();
        let Error::DuplicateFlagKey { flag, .. } = &err else {
            panic!("expected DuplicateFlagKey");
        };
        assert_eq!(flag, "-m");
    }

    #[test]
    fn flag_key_collision_across_scopes_is_fine() {
        // Defaults and a profile body are different scopes; sharing
        // a key across them is fine and is what makes profile
        // overrides work in the first place.
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
}
