//! Convert a resolved argument list into the output forms `jig`
//! needs:
//!
//! - [`to_argv`]: an `OsString` argv for `std::process::Command::args`
//!   (used by [`crate::exec`]).
//! - [`to_dry_run`]: a single shell-quoted line for `--dry-run` per
//!   `SPEC.md` §7.2.
//! - [`format_args`]: just the argument tokens, shell-quoted and
//!   space-joined (no program, no pass-through). Used by `--list`
//!   to render each command's default args per §7.1.
//!
//! Implements `SPEC.md` §2.5 (key prefix synthesis), §2.4.1 (`#true`
//! → bare flag, `#false` → suppressed at the resolve layer), §3.3
//! (pass-through trails the resolved args), and §7.2 (single-line
//! shell-quoted output).

use std::ffi::OsString;
use std::path::Path;

use crate::config::{Argument, FlagValue};
use crate::errors::{Error, Result};
use crate::resolve::EnvOp;

/// Produce the textual tokens for `args` per §2.5, omitting any
/// flag whose value is `Bool(false)` (defensive — resolve already
/// drops them).
fn args_to_tokens(args: &[Argument]) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::with_capacity(args.len() * 2);
    for arg in args {
        match arg {
            Argument::Flag {
                key,
                value: FlagValue::Bool(true),
                ..
            } => tokens.push(key.to_cli_flag()),
            Argument::Flag {
                value: FlagValue::Bool(false) | FlagValue::Null,
                ..
            } => {
                // `#false` and `#null` never emit. Resolve already
                // drops them; defensive no-op here.
            }
            Argument::Flag {
                key,
                value: FlagValue::Literal(s),
                ..
            } => {
                tokens.push(key.to_cli_flag());
                tokens.push(s.clone());
            }
            Argument::Positional(s) => tokens.push(s.clone()),
        }
    }
    tokens
}

/// Shell-quote each token via `shlex::try_quote` and join with
/// spaces. Returns [`Error::ArgumentContainsNul`] on values that
/// contain a NUL byte (which `shlex` cannot quote).
fn shell_join(tokens: &[String]) -> Result<String> {
    let mut out = String::new();
    for (i, t) in tokens.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let quoted =
            shlex::try_quote(t).map_err(|_| Error::ArgumentContainsNul { value: t.clone() })?;
        out.push_str(&quoted);
    }
    Ok(out)
}

/// Build the [`OsString`] argv vector for
/// `std::process::Command::args`. `args` should already have been
/// through [`crate::resolve`]; `#false` flags are skipped
/// defensively here in case any slip through.
#[must_use]
pub fn to_argv(args: &[Argument], passthrough: &[OsString]) -> Vec<OsString> {
    let mut out = Vec::with_capacity(args.len() * 2 + passthrough.len());
    for arg in args {
        match arg {
            Argument::Flag {
                key,
                value: FlagValue::Bool(true),
                ..
            } => out.push(OsString::from(key.to_cli_flag())),
            Argument::Flag {
                value: FlagValue::Bool(false) | FlagValue::Null,
                ..
            } => {
                // `#false` and `#null` never emit. Resolve already
                // drops them; defensive no-op here.
            }
            Argument::Flag {
                key,
                value: FlagValue::Literal(s),
                ..
            } => {
                out.push(OsString::from(key.to_cli_flag()));
                out.push(OsString::from(s));
            }
            Argument::Positional(s) => out.push(OsString::from(s)),
        }
    }
    out.extend(passthrough.iter().cloned());
    out
}

/// Build the single shell-quoted line emitted by `--dry-run`. Per
/// `SPEC.md` §7.2 the output must be copy-pasteable into a POSIX
/// shell. Per Q7, non-UTF-8 pass-through arguments error rather
/// than lossy-render.
///
/// When `env_ops` is non-empty, the line is prefixed with an
/// `env(1)` invocation that applies them: `-u NAME` for each unset
/// followed by `NAME=value` for each set, with values (but not
/// names) shell-quoted as needed.
///
/// When `cwd` is `Some`, the resolved command is wrapped in
/// `(cd <dir> && ...)` so chdir applies in a subshell and the
/// caller's shell is unaffected. The cwd subshell sits outside the
/// optional `env(1)` prefix per `SPEC.md` §7.2.
///
/// # Errors
///
/// Returns [`Error::PassthroughNotUtf8`] if a pass-through argument
/// is not valid UTF-8, or [`Error::ArgumentContainsNul`] if any
/// value contains a NUL byte (which `shlex` cannot quote).
pub fn to_dry_run(
    program: &str,
    args: &[Argument],
    passthrough: &[OsString],
    env_ops: &[EnvOp],
    cwd: Option<&Path>,
) -> Result<String> {
    let mut body_tokens = Vec::with_capacity(args.len() * 2 + passthrough.len() + 1);
    body_tokens.push(program.to_string());
    body_tokens.extend(args_to_tokens(args));
    for pt in passthrough {
        let s = pt
            .to_str()
            .ok_or_else(|| Error::PassthroughNotUtf8 {
                lossy: pt.to_string_lossy().into_owned(),
            })?
            .to_string();
        body_tokens.push(s);
    }
    let body = shell_join(&body_tokens)?;

    let env_parts = env_tokens_quoted(env_ops)?;
    let with_env = if env_parts.is_empty() {
        body
    } else {
        format!("env {} {body}", env_parts.join(" "))
    };

    match cwd {
        None => Ok(with_env),
        Some(dir) => {
            // The cwd path can be non-UTF-8 if the config-file
            // directory itself is non-UTF-8 (rare but possible on
            // Unix). A NUL byte cannot appear in an actual filesystem
            // path on Unix, but KDL `\u{0}` escapes can introduce one
            // into the string the user wrote for `cwd=`. Both are
            // surface-level rendering failures: the spawn itself does
            // not require UTF-8 quoting, only the preview does.
            let path = dir.to_str().ok_or_else(|| Error::CwdNotUtf8 {
                lossy: dir.to_string_lossy().into_owned(),
            })?;
            let quoted_dir = shlex::try_quote(path).map_err(|_| Error::CwdContainsNul {
                path: path.to_string(),
            })?;
            Ok(format!("(cd {quoted_dir} && {with_env})"))
        }
    }
}

/// Build the env-var prefix tokens (without the leading literal
/// `"env"` marker). Each unset emits `-u NAME`; each set emits
/// `NAME=value` with the value shell-quoted on its own (the entire
/// `K=V` token is *not* re-quoted, because `K=V` at the start of a
/// command is the canonical env-assignment form and a wrapping quote
/// would defeat it). Names are not quoted: validation enforces the
/// POSIX-portable identifier pattern (§2.9), which never needs
/// shell-quoting.
fn env_tokens_quoted(env_ops: &[EnvOp]) -> Result<Vec<String>> {
    let mut parts: Vec<String> = Vec::with_capacity(env_ops.len() * 2);
    // Unsets first, in source order.
    for op in env_ops {
        if let EnvOp::Unset { name } = op {
            parts.push("-u".to_string());
            parts.push(name.clone());
        }
    }
    // Sets second, in source order.
    for op in env_ops {
        if let EnvOp::Set { name, value } = op {
            let qv = shlex::try_quote(value)
                .map_err(|_| Error::ArgumentContainsNul {
                    value: value.clone(),
                })?
                .into_owned();
            parts.push(format!("{name}={qv}"));
        }
    }
    Ok(parts)
}

/// Render just the argument tokens (no program, no pass-through),
/// shell-quoted and space-joined. Used by `--list` to render each
/// command's default args per `SPEC.md` §7.1.
///
/// # Errors
///
/// Returns [`Error::ArgumentContainsNul`] if any value contains a
/// NUL byte.
pub fn format_args(args: &[Argument]) -> Result<String> {
    shell_join(&args_to_tokens(args))
}

/// Render env-var operations as a `--list` line: `-u UNSET NAME=value`,
/// space-joined. Used by `--list` per `SPEC.md` §7.1. Returns an
/// empty string for an empty input. Values that need shell-quoting
/// are quoted; names are not (they're POSIX-portable per §2.9).
///
/// # Errors
///
/// Returns [`Error::ArgumentContainsNul`] if any value contains a
/// NUL byte.
pub fn format_env(env_ops: &[EnvOp]) -> Result<String> {
    Ok(env_tokens_quoted(env_ops)?.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FlagKey, FlagMode, FlagValue};
    use miette::SourceSpan;

    fn flag(key: FlagKey, value: FlagValue) -> Argument {
        Argument::Flag {
            key,
            key_span: SourceSpan::from((0, 0)),
            value,
            mode: FlagMode::Plain,
        }
    }

    // --- to_argv ---

    #[test]
    fn argv_long_form_flag_with_value() {
        let args = vec![flag(
            FlagKey::Inferred("host".into()),
            FlagValue::Literal("0.0.0.0".into()),
        )];
        assert_eq!(
            to_argv(&args, &[]),
            vec![OsString::from("--host"), OsString::from("0.0.0.0")]
        );
    }

    #[test]
    fn argv_short_form_flag() {
        let args = vec![flag(
            FlagKey::Inferred("m".into()),
            FlagValue::Literal("/p".into()),
        )];
        assert_eq!(
            to_argv(&args, &[]),
            vec![OsString::from("-m"), OsString::from("/p")]
        );
    }

    #[test]
    fn argv_verbatim_key_passes_through() {
        let args = vec![flag(
            FlagKey::Verbatim("-ngl".into()),
            FlagValue::Literal("999".into()),
        )];
        assert_eq!(
            to_argv(&args, &[]),
            vec![OsString::from("-ngl"), OsString::from("999")]
        );
    }

    #[test]
    fn argv_bool_true_emits_only_key() {
        let args = vec![flag(
            FlagKey::Inferred("flash-attn".into()),
            FlagValue::Bool(true),
        )];
        assert_eq!(to_argv(&args, &[]), vec![OsString::from("--flash-attn")]);
    }

    #[test]
    fn argv_bool_false_is_suppressed() {
        let args = vec![flag(
            FlagKey::Inferred("foo".into()),
            FlagValue::Bool(false),
        )];
        assert!(to_argv(&args, &[]).is_empty());
    }

    #[test]
    fn argv_positionals_emit_verbatim() {
        let args = vec![
            Argument::Positional("input.mp4".into()),
            Argument::Positional("output.mp4".into()),
        ];
        assert_eq!(
            to_argv(&args, &[]),
            vec![OsString::from("input.mp4"), OsString::from("output.mp4")]
        );
    }

    #[test]
    fn argv_passthrough_appended_at_end() {
        let args = vec![flag(
            FlagKey::Inferred("host".into()),
            FlagValue::Literal("x".into()),
        )];
        let pt = vec![OsString::from("--extra"), OsString::from("y")];
        assert_eq!(
            to_argv(&args, &pt),
            vec![
                OsString::from("--host"),
                OsString::from("x"),
                OsString::from("--extra"),
                OsString::from("y"),
            ]
        );
    }

    // --- to_dry_run ---

    #[test]
    fn dry_run_basic() {
        let args = vec![flag(
            FlagKey::Inferred("host".into()),
            FlagValue::Literal("0.0.0.0".into()),
        )];
        let s = to_dry_run("llama-server", &args, &[], &[], None).unwrap();
        assert_eq!(s, "llama-server --host 0.0.0.0");
    }

    #[test]
    fn dry_run_quotes_spaces() {
        let args = vec![Argument::Positional("/path with spaces/file".into())];
        let s = to_dry_run("foo", &args, &[], &[], None).unwrap();
        // shlex picks single-quoting for values with shell-significant chars
        assert!(s.contains("'/path with spaces/file'"));
    }

    #[test]
    fn dry_run_quotes_glob_chars() {
        let args = vec![Argument::Positional("*.txt".into())];
        let s = to_dry_run("foo", &args, &[], &[], None).unwrap();
        assert!(s.contains("'*.txt'"));
    }

    #[test]
    fn dry_run_omits_value_for_bool_true() {
        let args = vec![flag(
            FlagKey::Inferred("flash-attn".into()),
            FlagValue::Bool(true),
        )];
        let s = to_dry_run("foo", &args, &[], &[], None).unwrap();
        assert_eq!(s, "foo --flash-attn");
    }

    #[test]
    fn dry_run_passthrough_quotes_when_needed() {
        let s = to_dry_run("foo", &[], &[OsString::from("a b")], &[], None).unwrap();
        assert_eq!(s, "foo 'a b'");
    }

    #[cfg(unix)]
    #[test]
    fn dry_run_errors_on_non_utf8_passthrough() {
        use std::os::unix::ffi::OsStringExt;
        let invalid = OsString::from_vec(vec![0xff, 0xfe]);
        let err = to_dry_run("foo", &[], &[invalid], &[], None).unwrap_err();
        assert!(matches!(err, Error::PassthroughNotUtf8 { .. }));
    }

    // --- format_args (used by --list per §7.1) ---

    #[test]
    fn format_args_omits_program_and_passthrough() {
        let args = vec![
            flag(
                FlagKey::Inferred("host".into()),
                FlagValue::Literal("0.0.0.0".into()),
            ),
            flag(
                FlagKey::Inferred("flash-attn".into()),
                FlagValue::Bool(true),
            ),
        ];
        assert_eq!(format_args(&args).unwrap(), "--host 0.0.0.0 --flash-attn");
    }

    #[test]
    fn format_args_empty_for_no_defaults() {
        assert_eq!(format_args(&[]).unwrap(), "");
    }

    // --- env-var rendering (--dry-run + --list) ---

    #[test]
    fn dry_run_with_env_set_only() {
        let env_ops = vec![
            EnvOp::Set {
                name: "A".into(),
                value: "1".into(),
            },
            EnvOp::Set {
                name: "B".into(),
                value: "two words".into(),
            },
        ];
        let s = to_dry_run("foo", &[], &[], &env_ops, None).unwrap();
        // The K= prefix is left bare so the assignment shape is
        // preserved; only the value is shell-quoted when needed.
        assert_eq!(s, "env A=1 B='two words' foo");
    }

    #[test]
    fn dry_run_with_env_unset_only() {
        let env_ops = vec![EnvOp::Unset { name: "OLD".into() }];
        let s = to_dry_run("foo", &[], &[], &env_ops, None).unwrap();
        assert_eq!(s, "env -u OLD foo");
    }

    #[test]
    fn dry_run_with_env_mixed_unsets_first() {
        let env_ops = vec![
            EnvOp::Set {
                name: "A".into(),
                value: "1".into(),
            },
            EnvOp::Unset { name: "OLD".into() },
            EnvOp::Set {
                name: "B".into(),
                value: "2".into(),
            },
        ];
        let s = to_dry_run("foo", &[], &[], &env_ops, None).unwrap();
        // Unsets always come before sets, irrespective of source
        // order, so `env(1)` parsing is unambiguous.
        assert_eq!(s, "env -u OLD A=1 B=2 foo");
    }

    #[test]
    fn dry_run_no_env_unchanged() {
        // Without env ops, the prefix is omitted entirely.
        let args = vec![flag(
            FlagKey::Inferred("host".into()),
            FlagValue::Literal("x".into()),
        )];
        let s = to_dry_run("foo", &args, &[], &[], None).unwrap();
        assert_eq!(s, "foo --host x");
    }

    #[test]
    fn format_env_drops_leading_env_marker() {
        // For --list, the `env:` label conveys what `env(1)` would,
        // so format_env strips the literal `env` prefix token.
        let env_ops = vec![
            EnvOp::Unset { name: "OLD".into() },
            EnvOp::Set {
                name: "A".into(),
                value: "1".into(),
            },
        ];
        assert_eq!(format_env(&env_ops).unwrap(), "-u OLD A=1");
    }

    #[test]
    fn format_env_empty_for_no_ops() {
        assert_eq!(format_env(&[]).unwrap(), "");
    }

    // --- §7.2 cwd subshell wrapping ---

    #[test]
    fn dry_run_with_cwd_only() {
        let args = vec![flag(
            FlagKey::Inferred("host".into()),
            FlagValue::Literal("0.0.0.0".into()),
        )];
        let s = to_dry_run(
            "llama-server",
            &args,
            &[],
            &[],
            Some(Path::new("/home/me/proj")),
        )
        .unwrap();
        assert_eq!(s, "(cd /home/me/proj && llama-server --host 0.0.0.0)");
    }

    #[test]
    fn dry_run_with_cwd_and_env() {
        // env(1) prefix sits inside the cwd subshell.
        let args = vec![flag(
            FlagKey::Inferred("host".into()),
            FlagValue::Literal("0.0.0.0".into()),
        )];
        let env_ops = vec![EnvOp::Set {
            name: "OLLAMA_HOST".into(),
            value: "0.0.0.0".into(),
        }];
        let s = to_dry_run(
            "llama-server",
            &args,
            &[],
            &env_ops,
            Some(Path::new("/home/me/proj")),
        )
        .unwrap();
        assert_eq!(
            s,
            "(cd /home/me/proj && env OLLAMA_HOST=0.0.0.0 llama-server --host 0.0.0.0)"
        );
    }

    #[test]
    fn dry_run_with_cwd_and_passthrough() {
        // Pass-through args still trail inside the subshell.
        let s = to_dry_run(
            "foo",
            &[],
            &[OsString::from("--extra"), OsString::from("a b")],
            &[],
            Some(Path::new("/proj")),
        )
        .unwrap();
        assert_eq!(s, "(cd /proj && foo --extra 'a b')");
    }

    #[test]
    fn dry_run_cwd_with_spaces_is_quoted() {
        let s = to_dry_run("foo", &[], &[], &[], Some(Path::new("/path with spaces"))).unwrap();
        assert_eq!(s, "(cd '/path with spaces' && foo)");
    }

    #[test]
    fn dry_run_without_cwd_unchanged() {
        // The cwd wrapper is omitted when None — output is byte
        // identical to the no-cwd path.
        let args = vec![flag(
            FlagKey::Inferred("host".into()),
            FlagValue::Literal("x".into()),
        )];
        let s = to_dry_run("foo", &args, &[], &[], None).unwrap();
        assert_eq!(s, "foo --host x");
    }
}
