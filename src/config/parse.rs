//! KDL document → [`Config`].
//!
//! Implements the structural rules from `SPEC.md`:
//!
//! - §2.2 (KDL format)
//! - §2.3 (argument vs profile distinguished by presence of children)
//! - §2.4 (flag = node-with-value; positional = node-without-value)
//! - §2.4.1 (boolean `#true` / `#false` vs string `"true"` / `"false"`)
//! - §2.4.2 (positionals starting with `-` use quoted node names)
//! - §2.5 (key with explicit dash → verbatim; otherwise → inferred)
//! - §2.6 (positionals)
//!
//! Constraint enforcement (`SPEC.md` §2.9) is the validator's job
//! and lands in Step 4. This module emits only structural errors:
//! flags with multiple values, KDL properties on any node, and
//! propagated `kdl::KdlError` syntax failures.

use kdl::{KdlDocument, KdlEntry, KdlNode, KdlValue};
use miette::NamedSource;

use super::{Argument, Command, CommandChild, Config, FlagKey, FlagValue};
use crate::errors::{Error, Result};

/// Parse a KDL document string into a [`Config`].
///
/// `path` is used as the diagnostic source name (e.g. `jig.kdl`)
/// and embedded in any `NamedSource`-bearing error variants.
///
/// # Errors
///
/// Returns [`Error::KdlParse`] if the input is not syntactically
/// valid KDL, [`Error::FlagMultipleValues`] if a flag node has
/// more than one value, or [`Error::NodeHasProperties`] if any
/// node carries a KDL property.
pub fn parse_str(content: &str, path: &str) -> Result<Config> {
    // The `kdl` crate parses KDL v2 by default and falls back to v1
    // when the `v1-fallback` feature is enabled (see Cargo.toml).
    let doc = KdlDocument::parse(content)?;
    let src = NamedSource::new(path, content.to_string());

    let mut commands = Vec::with_capacity(doc.nodes().len());
    for node in doc.nodes() {
        commands.push(parse_command(node, &src)?);
    }
    Ok(Config { commands })
}

fn parse_command(node: &KdlNode, src: &NamedSource<String>) -> Result<Command> {
    reject_properties(node, src)?;

    let name = node.name().value().to_string();

    // The command node may carry one optional value (the alias).
    // More than one value is a parse error.
    let values: Vec<&KdlEntry> = node
        .entries()
        .iter()
        .filter(|e| e.name().is_none())
        .collect();
    let alias = match values.as_slice() {
        [] => None,
        [entry] => Some(string_value(entry, src)?),
        [_, extra, ..] => {
            return Err(Error::FlagMultipleValues {
                src: src.clone(),
                span: extra.span(),
            });
        }
    };

    let children = match node.children() {
        Some(doc) => parse_command_children(doc, src)?,
        None => Vec::new(),
    };

    Ok(Command {
        name,
        alias,
        children,
    })
}

fn parse_command_children(
    doc: &KdlDocument,
    src: &NamedSource<String>,
) -> Result<Vec<CommandChild>> {
    let mut out = Vec::with_capacity(doc.nodes().len());
    for node in doc.nodes() {
        out.push(parse_command_child(node, src)?);
    }
    Ok(out)
}

fn parse_command_child(node: &KdlNode, src: &NamedSource<String>) -> Result<CommandChild> {
    if let Some(child_doc) = node.children() {
        // Has children → profile.
        reject_properties(node, src)?;
        // A profile node may not carry values of its own.
        if let Some(extra) = node.entries().iter().find(|e| e.name().is_none()) {
            return Err(Error::FlagMultipleValues {
                src: src.clone(),
                span: extra.span(),
            });
        }
        let name = node.name().value().to_string();
        let mut args = Vec::with_capacity(child_doc.nodes().len());
        for arg_node in child_doc.nodes() {
            args.push(parse_argument(arg_node, src)?);
        }
        Ok(CommandChild::Profile { name, args })
    } else {
        // No children → default argument (flag or positional).
        Ok(CommandChild::Default(parse_argument(node, src)?))
    }
}

fn parse_argument(node: &KdlNode, src: &NamedSource<String>) -> Result<Argument> {
    reject_properties(node, src)?;

    let values: Vec<&KdlEntry> = node
        .entries()
        .iter()
        .filter(|e| e.name().is_none())
        .collect();
    match values.as_slice() {
        [] => Ok(Argument::Positional(node.name().value().to_string())),
        [entry] => Ok(Argument::Flag {
            key: classify_flag_key(node.name().value()),
            value: flag_value(entry),
        }),
        [_first, extra, ..] => Err(Error::FlagMultipleValues {
            src: src.clone(),
            span: extra.span(),
        }),
    }
}

fn classify_flag_key(raw: &str) -> FlagKey {
    if raw.starts_with('-') {
        FlagKey::Verbatim(raw.to_string())
    } else {
        FlagKey::Inferred(raw.to_string())
    }
}

fn flag_value(entry: &KdlEntry) -> FlagValue {
    match entry.value() {
        KdlValue::Bool(b) => FlagValue::Bool(*b),
        KdlValue::String(s) => FlagValue::Literal(s.clone()),
        // For Integer / Float / Null, prefer the original source
        // representation (e.g. `0.5` round-trips exactly) when the
        // entry preserved it; otherwise fall back to `Display`,
        // which produces canonical KDL form. See IMPLEMENTATION.md
        // §7.2.
        other => FlagValue::Literal(
            entry
                .format()
                .map(|f| f.value_repr.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| other.to_string()),
        ),
    }
}

fn string_value(entry: &KdlEntry, src: &NamedSource<String>) -> Result<String> {
    match entry.value() {
        KdlValue::String(s) => Ok(s.clone()),
        _ => Err(Error::FlagMultipleValues {
            // Re-using FlagMultipleValues here would be wrong; aliases
            // must be strings. We don't yet have a dedicated variant
            // for "expected string"; for now, surface as a generic
            // shape error pointing at the offending entry.
            //
            // TODO(step 4): introduce `Error::ExpectedString` once
            // the validator's error vocabulary lands.
            src: src.clone(),
            span: entry.span(),
        }),
    }
}

fn reject_properties(node: &KdlNode, src: &NamedSource<String>) -> Result<()> {
    if let Some(prop) = node.entries().iter().find(|e| e.name().is_some()) {
        return Err(Error::NodeHasProperties {
            src: src.clone(),
            span: prop.span(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Config> {
        parse_str(s, "test.kdl")
    }

    #[test]
    fn empty_document() {
        let cfg = parse("").unwrap();
        assert!(cfg.commands.is_empty());
    }

    #[test]
    fn command_with_no_body() {
        let cfg = parse("foo\n").unwrap();
        assert_eq!(cfg.commands.len(), 1);
        let cmd = &cfg.commands[0];
        assert_eq!(cmd.name, "foo");
        assert_eq!(cmd.alias, None);
        assert!(cmd.children.is_empty());
    }

    #[test]
    fn command_with_alias() {
        let cfg = parse(r#"llama-server "serve" {}"#).unwrap();
        let cmd = &cfg.commands[0];
        assert_eq!(cmd.name, "llama-server");
        assert_eq!(cmd.alias.as_deref(), Some("serve"));
    }

    #[test]
    fn flag_two_char_key_is_inferred_long() {
        let cfg = parse(r#"foo { host "0.0.0.0" }"#).unwrap();
        let CommandChild::Default(Argument::Flag { key, value }) = &cfg.commands[0].children[0]
        else {
            panic!("expected default flag");
        };
        assert_eq!(*key, FlagKey::Inferred("host".to_string()));
        assert_eq!(*value, FlagValue::Literal("0.0.0.0".to_string()));
    }

    #[test]
    fn flag_one_char_key_stays_inferred() {
        let cfg = parse(r#"foo { m "/path" }"#).unwrap();
        let CommandChild::Default(Argument::Flag { key, .. }) = &cfg.commands[0].children[0] else {
            panic!("expected default flag");
        };
        // §2.5's `-` vs `--` synthesis is `format`'s job; parser
        // just records that this came in without an explicit dash.
        assert_eq!(*key, FlagKey::Inferred("m".to_string()));
    }

    #[test]
    fn flag_with_explicit_dash_is_verbatim() {
        let cfg = parse(r"foo { -ngl 999 }").unwrap();
        let CommandChild::Default(Argument::Flag { key, value }) = &cfg.commands[0].children[0]
        else {
            panic!("expected default flag");
        };
        assert_eq!(*key, FlagKey::Verbatim("-ngl".to_string()));
        assert_eq!(*value, FlagValue::Literal("999".to_string()));
    }

    #[test]
    fn boolean_flag_true_and_false() {
        let cfg = parse(
            r"foo {
                flash-attn #true
                disabled #false
            }",
        )
        .unwrap();
        let CommandChild::Default(Argument::Flag { value: v1, .. }) = &cfg.commands[0].children[0]
        else {
            panic!();
        };
        let CommandChild::Default(Argument::Flag { value: v2, .. }) = &cfg.commands[0].children[1]
        else {
            panic!();
        };
        assert_eq!(*v1, FlagValue::Bool(true));
        assert_eq!(*v2, FlagValue::Bool(false));
    }

    #[test]
    fn string_true_is_literal_not_boolean() {
        // §2.4.1: `"true"` is a literal value, distinct from `#true`.
        let cfg = parse(r#"foo { test "true" }"#).unwrap();
        let CommandChild::Default(Argument::Flag { value, .. }) = &cfg.commands[0].children[0]
        else {
            panic!();
        };
        assert_eq!(*value, FlagValue::Literal("true".to_string()));
    }

    #[test]
    fn integer_value_uses_source_repr() {
        let cfg = parse(r"foo { port 8090 }").unwrap();
        let CommandChild::Default(Argument::Flag { value, .. }) = &cfg.commands[0].children[0]
        else {
            panic!();
        };
        assert_eq!(*value, FlagValue::Literal("8090".to_string()));
    }

    #[test]
    fn float_value_uses_source_repr() {
        let cfg = parse(r"foo { ratio 0.5 }").unwrap();
        let CommandChild::Default(Argument::Flag { value, .. }) = &cfg.commands[0].children[0]
        else {
            panic!();
        };
        assert_eq!(*value, FlagValue::Literal("0.5".to_string()));
    }

    #[test]
    fn positional_no_value() {
        // §2.4: a node with no value is a positional. The node name
        // (here, the quoted string "output.mp4") is the literal
        // positional value.
        let cfg = parse(
            r#"ffmpeg {
                profile {
                    "output.mp4"
                }
            }"#,
        )
        .unwrap();
        let CommandChild::Profile { args, .. } = &cfg.commands[0].children[0] else {
            panic!();
        };
        let Argument::Positional(s) = &args[0] else {
            panic!();
        };
        assert_eq!(s, "output.mp4");
    }

    #[test]
    fn positional_starting_with_dash_quoted() {
        let cfg = parse(
            r#"foo {
                profile {
                    "--"
                    "-stdin"
                }
            }"#,
        )
        .unwrap();
        let CommandChild::Profile { args, .. } = &cfg.commands[0].children[0] else {
            panic!();
        };
        let Argument::Positional(a) = &args[0] else {
            panic!();
        };
        let Argument::Positional(b) = &args[1] else {
            panic!();
        };
        assert_eq!(a, "--");
        assert_eq!(b, "-stdin");
    }

    #[test]
    fn profile_distinguished_by_child_block() {
        // A node with `{}` is a profile (even an empty block), per
        // the structural rule in §2.3.
        let cfg = parse(
            r#"foo {
                default-flag "x"
                empty-profile {}
                with-flags {
                    timeout 10
                }
            }"#,
        )
        .unwrap();
        let cmd = &cfg.commands[0];
        assert!(matches!(cmd.children[0], CommandChild::Default(_)));
        assert!(matches!(
            cmd.children[1],
            CommandChild::Profile { ref name, .. } if name == "empty-profile"
        ));
        assert!(matches!(
            cmd.children[2],
            CommandChild::Profile { ref name, .. } if name == "with-flags"
        ));
    }

    #[test]
    fn source_order_preserved() {
        // §2.7: defaults and profiles are interleaved in source
        // order. The parser must preserve that order in `children`.
        let cfg = parse(
            r#"foo {
                "a-positional"
                profile-a {}
                verbose #true
                profile-b {}
            }"#,
        )
        .unwrap();
        let cmd = &cfg.commands[0];
        assert_eq!(cmd.children.len(), 4);
        assert!(
            matches!(&cmd.children[0], CommandChild::Default(Argument::Positional(s)) if s == "a-positional")
        );
        assert!(
            matches!(&cmd.children[1], CommandChild::Profile { name, .. } if name == "profile-a")
        );
        assert!(matches!(
            &cmd.children[2],
            CommandChild::Default(Argument::Flag { .. })
        ));
        assert!(
            matches!(&cmd.children[3], CommandChild::Profile { name, .. } if name == "profile-b")
        );
    }

    #[test]
    fn flag_with_multiple_values_errors() {
        let err = parse(r#"foo { host "a" "b" }"#).unwrap_err();
        assert!(matches!(err, Error::FlagMultipleValues { .. }));
    }

    #[test]
    fn property_on_flag_errors() {
        let err = parse(r"foo { host port=8090 }").unwrap_err();
        assert!(matches!(err, Error::NodeHasProperties { .. }));
    }

    #[test]
    fn property_on_positional_errors() {
        let err = parse(r"foo { positional ignored=1 }").unwrap_err();
        assert!(matches!(err, Error::NodeHasProperties { .. }));
    }

    #[test]
    fn property_on_command_errors() {
        let err = parse(r"foo extra=1 {}").unwrap_err();
        assert!(matches!(err, Error::NodeHasProperties { .. }));
    }

    #[test]
    fn syntactically_invalid_kdl_errors() {
        let err = parse("foo {").unwrap_err();
        assert!(matches!(err, Error::KdlParse(_)));
    }

    #[test]
    fn spec_5_1_llama_server_round_trips() {
        let cfg = parse(
            r#"llama-server "serve" {
                host "0.0.0.0"
                port 8090
                c 32768
                flash-attn #true

                qwen-coder {
                    m "/models/qwen-coder.gguf"
                    -ngl 999
                    -ts "0.5,0.5"
                }

                llama3 {
                    m "/models/llama3.gguf"
                    port 8091
                }
            }"#,
        )
        .unwrap();
        let cmd = &cfg.commands[0];
        assert_eq!(cmd.name, "llama-server");
        assert_eq!(cmd.alias.as_deref(), Some("serve"));
        // 4 defaults + 2 profiles
        assert_eq!(cmd.children.len(), 6);
    }

    #[test]
    fn spec_5_3_1_ffmpeg_interleaved() {
        let cfg = parse(
            r#"ffmpeg "transcode" {
                h264 {
                    i "input.mp4"
                    -c:v "libx264"
                    "output.mp4"
                }
            }"#,
        )
        .unwrap();
        let CommandChild::Profile { args, .. } = &cfg.commands[0].children[0] else {
            panic!();
        };
        // Interleaved: flag, flag, positional in source order.
        assert!(matches!(&args[0], Argument::Flag { .. }));
        assert!(matches!(&args[1], Argument::Flag { .. }));
        assert!(matches!(args[2], Argument::Positional(_)));
    }
}
