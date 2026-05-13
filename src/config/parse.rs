//! KDL document → [`Config`].
//!
//! Implements the structural rules from `SPEC.md`:
//!
//! - §2.2 (KDL format)
//! - §2.3 (argument vs profile distinguished by presence of children)
//! - §2.4 (flag = node-with-value; positional = node-without-value)
//! - §2.4.1 (boolean `#true` / `#false` vs string `"true"` / `"false"`)
//! - §2.4.2 (positionals starting with `-` use quoted node names)
//! - §2.5 (key with explicit dash → verbatim; otherwise → inferred;
//!   leading `+` on a flag key is the explicit append marker)
//! - §2.6 (positionals)
//! - §2.10 (`(env)` type annotation declares an env-var contribution
//!   on a node inside a command body or profile body)
//!
//! Constraint enforcement (`SPEC.md` §2.9) is the validator's job.
//! This module emits only structural errors: flags with multiple
//! values, KDL properties on any node, an empty key after stripping
//! a `+` marker, propagated `kdl::KdlError` syntax failures, and
//! the env-shape rejections listed in §2.10 (children on an `(env)`
//! node, missing or invalid value, `+` marker, unknown annotation).

use kdl::{KdlDocument, KdlEntry, KdlNode, KdlValue};
use miette::NamedSource;

use super::{
    Argument, Command, CommandChild, Config, EnvEntry, EnvValue, FlagKey, FlagMode, FlagValue,
};
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
    // Top-level nodes are commands; no type annotation is meaningful
    // here, so reject any (`(env)` or otherwise).
    reject_any_annotation(node, src)?;

    let name = node.name().value().to_string();
    let name_span = node.name().span();

    // The command node may carry one optional value (the alias).
    // More than one value is a parse error.
    let values: Vec<&KdlEntry> = node
        .entries()
        .iter()
        .filter(|e| e.name().is_none())
        .collect();
    let (alias, alias_span) = match values.as_slice() {
        [] => (None, None),
        [entry] => (Some(string_value(entry, src)?), Some(entry.span())),
        [_, extra, ..] => {
            return Err(Error::FlagMultipleValues {
                src: src.clone(),
                span: extra.span(),
            });
        }
    };

    let (children, env) = match node.children() {
        Some(doc) => parse_command_body(doc, src)?,
        None => (Vec::new(), Vec::new()),
    };

    Ok(Command {
        name,
        name_span,
        alias,
        alias_span,
        children,
        env,
    })
}

/// Parse the body of a command node — defaults, profiles, and
/// `(env)` declarations all interleaved. Returns the source-ordered
/// `children` list (defaults + profiles) and the source-ordered
/// `env` list separately, since env contributions travel on a
/// parallel channel from argv (`SPEC.md` §2.10 / §2.11).
fn parse_command_body(
    doc: &KdlDocument,
    src: &NamedSource<String>,
) -> Result<(Vec<CommandChild>, Vec<EnvEntry>)> {
    let mut children: Vec<CommandChild> = Vec::with_capacity(doc.nodes().len());
    let mut env: Vec<EnvEntry> = Vec::new();
    for node in doc.nodes() {
        match classify_annotation(node, src)? {
            AnnotationKind::Env => {
                // `(env)` only makes sense on a node-without-children
                // (it carries a single value or `#false`).
                if node.children().is_some() {
                    return Err(Error::EnvOnNodeWithChildren {
                        src: src.clone(),
                        span: node.name().span(),
                    });
                }
                env.push(parse_env_entry(node, src)?);
            }
            AnnotationKind::None => {
                if let Some(child_doc) = node.children() {
                    // Has children → profile. Reject positional values
                    // on profile nodes (they have no v1 meaning); allow
                    // only the named property `extends="<parent>"` per
                    // `SPEC.md` §2.8.5.
                    if let Some(extra) = node.entries().iter().find(|e| e.name().is_none()) {
                        return Err(Error::FlagMultipleValues {
                            src: src.clone(),
                            span: extra.span(),
                        });
                    }
                    let extends = parse_profile_extends(node, src)?;
                    let name = node.name().value().to_string();
                    let name_span = node.name().span();
                    let (args, profile_env) = parse_profile_body(child_doc, src)?;
                    children.push(CommandChild::Profile {
                        name,
                        name_span,
                        extends,
                        args,
                        env: profile_env,
                    });
                } else {
                    // No children, no annotation → default argument.
                    children.push(CommandChild::Default(parse_argument(node, src)?));
                }
            }
        }
    }
    Ok((children, env))
}

/// Parse the inheritance pointer from a profile node's properties.
/// Per `SPEC.md` §2.8.5 a profile may carry exactly one named
/// property `extends="<parent>"` where the value is a string; any
/// other property is rejected. Returns `None` when no `extends`
/// property is present.
fn parse_profile_extends(
    node: &KdlNode,
    src: &NamedSource<String>,
) -> Result<Option<(String, miette::SourceSpan)>> {
    let mut extends: Option<(String, miette::SourceSpan)> = None;
    for entry in node.entries() {
        let Some(prop_name) = entry.name() else {
            // Unnamed (positional) entries are rejected by the caller.
            continue;
        };
        if prop_name.value() != "extends" {
            return Err(Error::UnsupportedPropertyOnProfile {
                name: prop_name.value().to_string(),
                src: src.clone(),
                span: entry.span(),
            });
        }
        if let Some((_, first)) = &extends {
            return Err(Error::DuplicateProfileExtends {
                src: src.clone(),
                first: *first,
                second: entry.span(),
            });
        }
        let KdlValue::String(parent) = entry.value() else {
            return Err(Error::ProfileExtendsBadValue {
                src: src.clone(),
                span: entry.span(),
            });
        };
        extends = Some((parent.clone(), entry.span()));
    }
    Ok(extends)
}

/// Parse the body of a profile node — arguments and `(env)`
/// declarations interleaved. Profiles do not nest in v1; a node
/// with children inside a profile body falls through to the
/// argument path (its children are silently ignored, matching
/// the pre-§2.10 behavior).
fn parse_profile_body(
    doc: &KdlDocument,
    src: &NamedSource<String>,
) -> Result<(Vec<Argument>, Vec<EnvEntry>)> {
    let mut args: Vec<Argument> = Vec::with_capacity(doc.nodes().len());
    let mut env: Vec<EnvEntry> = Vec::new();
    for node in doc.nodes() {
        match classify_annotation(node, src)? {
            AnnotationKind::Env => {
                if node.children().is_some() {
                    return Err(Error::EnvOnNodeWithChildren {
                        src: src.clone(),
                        span: node.name().span(),
                    });
                }
                env.push(parse_env_entry(node, src)?);
            }
            AnnotationKind::None => args.push(parse_argument(node, src)?),
        }
    }
    Ok((args, env))
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
        [entry] => {
            // §2.5: a leading `+` on a flag key is the explicit append
            // marker. Strip it and apply the dash-prefix rules to the
            // remainder. The marker is flag-only — positional handling
            // above takes the node name verbatim.
            let raw = node.name().value();
            let (mode, key_text) = if let Some(rest) = raw.strip_prefix('+') {
                if rest.is_empty() {
                    return Err(Error::EmptyKeyAfterMarker {
                        src: src.clone(),
                        span: node.name().span(),
                    });
                }
                (FlagMode::Append, rest)
            } else {
                (FlagMode::Plain, raw)
            };
            let value = flag_value(entry);
            // §2.4.3: `#null` is a position-only placeholder that
            // never emits. Combining it with the `+` marker (which
            // requests a separate own-position emission) is
            // meaningless; reject so the user is steered toward
            // either dropping the marker or using a real value.
            if mode == FlagMode::Append && matches!(value, FlagValue::Null) {
                return Err(Error::NullWithAppendMarker {
                    src: src.clone(),
                    span: node.name().span(),
                });
            }
            Ok(Argument::Flag {
                key: classify_flag_key(key_text),
                key_span: node.name().span(),
                value,
                mode,
            })
        }
        [_first, extra, ..] => Err(Error::FlagMultipleValues {
            src: src.clone(),
            span: extra.span(),
        }),
    }
}

/// Build an [`EnvEntry`] from a node bearing the `(env)` annotation.
/// The caller guarantees `node.children().is_none()` and the
/// annotation is `(env)`.
fn parse_env_entry(node: &KdlNode, src: &NamedSource<String>) -> Result<EnvEntry> {
    reject_properties(node, src)?;

    // The `+` append marker is meaningless for env vars (POSIX
    // assigns one value per name); reject before stripping.
    let raw = node.name().value();
    if raw.starts_with('+') {
        return Err(Error::EnvWithAppendMarker {
            src: src.clone(),
            span: node.name().span(),
        });
    }

    let values: Vec<&KdlEntry> = node
        .entries()
        .iter()
        .filter(|e| e.name().is_none())
        .collect();
    let entry = match values.as_slice() {
        [] => {
            return Err(Error::EnvNoValue {
                src: src.clone(),
                span: node.name().span(),
            });
        }
        [entry] => *entry,
        [_first, extra, ..] => {
            return Err(Error::EnvMultipleValues {
                src: src.clone(),
                span: extra.span(),
            });
        }
    };

    let value = match entry.value() {
        KdlValue::Bool(false) => EnvValue::Unset,
        KdlValue::Bool(true) | KdlValue::Null => {
            return Err(Error::EnvInvalidValue {
                src: src.clone(),
                span: entry.span(),
            });
        }
        KdlValue::String(s) => EnvValue::Set(s.clone()),
        other => EnvValue::Set(
            entry
                .format()
                .map(|f| f.value_repr.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| other.to_string()),
        ),
    };

    Ok(EnvEntry {
        name: raw.to_string(),
        name_span: node.name().span(),
        value,
    })
}

/// Annotation routing for command-body / profile-body nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnnotationKind {
    None,
    Env,
}

/// Classify a node's `(ty)` annotation. `None` and `(env)` are the
/// two recognized states inside a command or profile body. Any
/// other annotation produces an [`Error::UnknownTypeAnnotation`].
fn classify_annotation(node: &KdlNode, src: &NamedSource<String>) -> Result<AnnotationKind> {
    match node.ty() {
        None => Ok(AnnotationKind::None),
        Some(id) if id.value() == "env" => Ok(AnnotationKind::Env),
        Some(id) => Err(Error::UnknownTypeAnnotation {
            annotation: id.value().to_string(),
            src: src.clone(),
            span: id.span(),
        }),
    }
}

/// Reject any type annotation on a node where none is meaningful
/// (top-level commands).
fn reject_any_annotation(node: &KdlNode, src: &NamedSource<String>) -> Result<()> {
    if let Some(id) = node.ty() {
        return Err(Error::UnknownTypeAnnotation {
            annotation: id.value().to_string(),
            src: src.clone(),
            span: id.span(),
        });
    }
    Ok(())
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
        KdlValue::Null => FlagValue::Null,
        KdlValue::String(s) => FlagValue::Literal(s.clone()),
        // For Integer / Float, prefer the original source
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
        _ => Err(Error::ExpectedString {
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
        let CommandChild::Default(Argument::Flag { key, value, .. }) = &cfg.commands[0].children[0]
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
        let CommandChild::Default(Argument::Flag { key, value, .. }) = &cfg.commands[0].children[0]
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
    fn append_marker_one_char_key() {
        let cfg = parse(r#"foo { +I "/proj" }"#).unwrap();
        let CommandChild::Default(Argument::Flag { key, mode, .. }) = &cfg.commands[0].children[0]
        else {
            panic!("expected default flag");
        };
        // Marker is stripped; `I` is 1 char → resolves to `-I`.
        assert_eq!(*key, FlagKey::Inferred("I".to_string()));
        assert_eq!(*mode, super::FlagMode::Append);
    }

    #[test]
    fn append_marker_long_key() {
        let cfg = parse(r#"foo { +host "0.0.0.0" }"#).unwrap();
        let CommandChild::Default(Argument::Flag { key, mode, .. }) = &cfg.commands[0].children[0]
        else {
            panic!("expected default flag");
        };
        assert_eq!(*key, FlagKey::Inferred("host".to_string()));
        assert_eq!(*mode, super::FlagMode::Append);
    }

    #[test]
    fn append_marker_with_explicit_dash_remains_verbatim() {
        // `+-ngl 999` strips the `+`; the remaining `-ngl` is a
        // verbatim key per §2.5 rule 1, just with the marker set.
        let cfg = parse(r"foo { +-ngl 999 }").unwrap();
        let CommandChild::Default(Argument::Flag { key, mode, .. }) = &cfg.commands[0].children[0]
        else {
            panic!("expected default flag");
        };
        assert_eq!(*key, FlagKey::Verbatim("-ngl".to_string()));
        assert_eq!(*mode, super::FlagMode::Append);
    }

    #[test]
    fn append_marker_alone_rejected() {
        // `+` with nothing after is empty key; we error rather than
        // silently producing a flag with key `""`.
        let err = parse(r#"foo { "+" "v" }"#).unwrap_err();
        assert!(matches!(err, Error::EmptyKeyAfterMarker { .. }));
    }

    #[test]
    fn unmarked_flag_has_plain_mode() {
        let cfg = parse(r#"foo { host "x" }"#).unwrap();
        let CommandChild::Default(Argument::Flag { mode, .. }) = &cfg.commands[0].children[0]
        else {
            panic!("expected default flag");
        };
        assert_eq!(*mode, super::FlagMode::Plain);
    }

    #[test]
    fn plus_on_positional_is_part_of_value() {
        // A node with no value is a positional. The leading `+` is
        // part of the literal positional value, not the marker.
        let cfg = parse(
            r#"foo {
                profile {
                    "+x"
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
        assert_eq!(s, "+x");
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

    // --- §2.10 env-var declarations ---

    #[test]
    fn env_string_value_in_defaults() {
        let cfg = parse(r#"foo { (env)OLLAMA_HOST "0.0.0.0" }"#).unwrap();
        let cmd = &cfg.commands[0];
        assert_eq!(cmd.env.len(), 1);
        assert_eq!(cmd.env[0].name, "OLLAMA_HOST");
        assert_eq!(cmd.env[0].value, EnvValue::Set("0.0.0.0".to_string()));
        // The argv-side children list does not see env declarations.
        assert!(cmd.children.is_empty());
    }

    #[test]
    fn env_integer_value_uses_source_repr() {
        let cfg = parse(r"foo { (env)PORT 8090 }").unwrap();
        let cmd = &cfg.commands[0];
        assert_eq!(cmd.env[0].value, EnvValue::Set("8090".to_string()));
    }

    #[test]
    fn env_float_value_uses_source_repr() {
        let cfg = parse(r"foo { (env)RATIO 0.5 }").unwrap();
        let cmd = &cfg.commands[0];
        assert_eq!(cmd.env[0].value, EnvValue::Set("0.5".to_string()));
    }

    #[test]
    fn env_false_means_unset() {
        let cfg = parse(r"foo { (env)FOO #false }").unwrap();
        let cmd = &cfg.commands[0];
        assert_eq!(cmd.env[0].value, EnvValue::Unset);
    }

    #[test]
    fn env_in_profile_body() {
        let cfg = parse(
            r#"foo {
                bar {
                    (env)BAZ "1"
                    (env)QUX #false
                    flag "v"
                }
            }"#,
        )
        .unwrap();
        let CommandChild::Profile { args, env, .. } = &cfg.commands[0].children[0] else {
            panic!("expected profile");
        };
        assert_eq!(env.len(), 2);
        assert_eq!(env[0].name, "BAZ");
        assert_eq!(env[0].value, EnvValue::Set("1".to_string()));
        assert_eq!(env[1].name, "QUX");
        assert_eq!(env[1].value, EnvValue::Unset);
        // The profile's argv-side args contain only the flag.
        assert_eq!(args.len(), 1);
    }

    #[test]
    fn env_with_no_value_errors() {
        let err = parse(r"foo { (env)BARE }").unwrap_err();
        assert!(matches!(err, Error::EnvNoValue { .. }));
    }

    #[test]
    fn env_true_value_errors() {
        let err = parse(r"foo { (env)FOO #true }").unwrap_err();
        assert!(matches!(err, Error::EnvInvalidValue { .. }));
    }

    #[test]
    fn env_null_value_errors() {
        let err = parse(r"foo { (env)FOO #null }").unwrap_err();
        assert!(matches!(err, Error::EnvInvalidValue { .. }));
    }

    #[test]
    fn env_with_children_errors() {
        let err = parse(
            r#"foo {
                (env)FOO "x" {
                    inner "y"
                }
            }"#,
        )
        .unwrap_err();
        assert!(matches!(err, Error::EnvOnNodeWithChildren { .. }));
    }

    #[test]
    fn env_with_children_no_value_errors_with_children_diagnostic() {
        // A profile-shaped form (children but no value) on an env
        // declaration must still surface as `EnvOnNodeWithChildren`,
        // not `EnvNoValue` — the user's intent is closer to "I tried
        // to nest a profile here" than "I forgot a value".
        let err = parse(
            r"foo {
                (env)FOO {
                    inner
                }
            }",
        )
        .unwrap_err();
        assert!(matches!(err, Error::EnvOnNodeWithChildren { .. }));
    }

    #[test]
    fn env_multiple_values_errors() {
        let err = parse(r#"foo { (env)FOO "a" "b" }"#).unwrap_err();
        assert!(matches!(err, Error::EnvMultipleValues { .. }));
    }

    #[test]
    fn env_with_append_marker_errors() {
        // `+` on an env declaration is meaningless; reject.
        let err = parse(r#"foo { (env)+FOO "x" }"#).unwrap_err();
        assert!(matches!(err, Error::EnvWithAppendMarker { .. }));
    }

    #[test]
    fn unknown_annotation_errors() {
        let err = parse(r#"foo { (cwd)dir "/path" }"#).unwrap_err();
        assert!(matches!(
            err,
            Error::UnknownTypeAnnotation { ref annotation, .. } if annotation == "cwd"
        ));
    }

    #[test]
    fn annotation_on_top_level_command_errors() {
        let err = parse(r#"(env)foo "x" {}"#).unwrap_err();
        assert!(matches!(err, Error::UnknownTypeAnnotation { .. }));
    }

    #[test]
    fn env_alongside_flags_in_same_body() {
        // Source-order interleaving of flags and env decls works:
        // the parser sorts them into separate vecs but each retains
        // its own intra-list order.
        let cfg = parse(
            r#"foo {
                host "0.0.0.0"
                (env)A "1"
                port 8090
                (env)B "2"
            }"#,
        )
        .unwrap();
        let cmd = &cfg.commands[0];
        // Two flag defaults, two env entries; profiles is empty.
        assert_eq!(cmd.children.len(), 2);
        assert_eq!(cmd.env.len(), 2);
        assert_eq!(cmd.env[0].name, "A");
        assert_eq!(cmd.env[1].name, "B");
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

    // --- §2.4.3 `#null` placeholder ---

    #[test]
    fn null_value_parses_as_flag_value_null() {
        let cfg = parse(r"foo { a #null }").unwrap();
        let CommandChild::Default(Argument::Flag { value, .. }) = &cfg.commands[0].children[0]
        else {
            panic!("expected default flag");
        };
        assert_eq!(*value, FlagValue::Null);
    }

    #[test]
    fn quoted_hash_null_is_literal_string_not_placeholder() {
        // §2.4.1's boolean-vs-string distinction extends to null:
        // `"#null"` is a literal string value, not the `#null` keyword.
        let cfg = parse("foo { a \"#null\" }").unwrap();
        let CommandChild::Default(Argument::Flag { value, .. }) = &cfg.commands[0].children[0]
        else {
            panic!("expected default flag");
        };
        assert_eq!(*value, FlagValue::Literal("#null".to_string()));
    }

    #[test]
    fn null_with_append_marker_rejected() {
        let err = parse(r"foo { +a #null }").unwrap_err();
        assert!(matches!(err, Error::NullWithAppendMarker { .. }));
    }

    #[test]
    fn null_without_marker_accepted() {
        // Sanity-check the negative test above: plain `a #null` is
        // fine; only `+a #null` is the rejected combination.
        parse(r"foo { a #null }").unwrap();
    }

    // --- §2.8.5 `extends` property on profile nodes ---

    #[test]
    fn profile_with_extends_property_captured() {
        let cfg = parse(
            r#"foo {
                parent { x "1" }
                child extends="parent" { y "2" }
            }"#,
        )
        .unwrap();
        let CommandChild::Profile {
            name,
            extends,
            args,
            ..
        } = &cfg.commands[0].children[1]
        else {
            panic!("expected profile at index 1");
        };
        assert_eq!(name, "child");
        let (parent, _span) = extends.as_ref().expect("extends should be set");
        assert_eq!(parent, "parent");
        // Body still parses normally.
        assert_eq!(args.len(), 1);
    }

    #[test]
    fn profile_without_extends_keeps_none() {
        let cfg = parse(r"foo { fast { timeout 5 } }").unwrap();
        let CommandChild::Profile { extends, .. } = &cfg.commands[0].children[0] else {
            panic!();
        };
        assert!(extends.is_none());
    }

    #[test]
    fn profile_extends_non_string_rejected() {
        for snippet in [
            r"foo { child extends=#true {} }",
            r"foo { child extends=42 {} }",
            r"foo { child extends=#false {} }",
        ] {
            let err = parse(snippet).unwrap_err();
            assert!(
                matches!(err, Error::ProfileExtendsBadValue { .. }),
                "expected ProfileExtendsBadValue for {snippet:?}, got {err:?}",
            );
        }
    }

    #[test]
    fn profile_with_unknown_property_rejected() {
        let err = parse(r#"foo { child base="parent" {} }"#).unwrap_err();
        let Error::UnsupportedPropertyOnProfile { name, .. } = err else {
            panic!("expected UnsupportedPropertyOnProfile");
        };
        assert_eq!(name, "base");
    }

    #[test]
    fn profile_with_duplicate_extends_rejected() {
        let err = parse(r#"foo { child extends="a" extends="b" {} }"#).unwrap_err();
        assert!(matches!(err, Error::DuplicateProfileExtends { .. }));
    }

    #[test]
    fn profile_with_positional_value_still_rejected() {
        // A positional ("value") on a profile node remains a parse
        // error — only `extends="<parent>"` is allowed.
        let err = parse(r#"foo { child "parent" {} }"#).unwrap_err();
        assert!(matches!(err, Error::FlagMultipleValues { .. }));
    }
}
