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

use crate::config::{Argument, EnvEntry, EnvValue, FlagValue};
use crate::errors::{Error, Result};
use crate::resolve::EnvOp;

/// Produce the textual tokens for `args` per §2.5, omitting any
/// flag whose value is `Bool(false)` (defensive — resolve already
/// drops them). Takes any borrowed iterator over [`Argument`] so
/// callers (notably [`crate::list`]) don't need to materialize an
/// owned `Vec<Argument>` just to invoke the renderer.
fn args_to_tokens<'a, I>(args: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a Argument>,
{
    let iter = args.into_iter();
    let (lower, _) = iter.size_hint();
    let mut tokens: Vec<String> = Vec::with_capacity(lower * 2);
    for arg in iter {
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

    let env_parts = env_tokens_quoted(env_ops_as_pairs(env_ops))?;
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
///
/// The input is an iterator of `(name, value)` pairs where `value`
/// is `None` for an unset and `Some(...)` for a set. This lets both
/// the resolved-env path ([`EnvOp`], post-merge) and the config-side
/// path ([`crate::config::EnvEntry`], for `--list`) feed the same
/// renderer without materializing an intermediate `Vec<EnvOp>`.
fn env_tokens_quoted<'a, I>(env: I) -> Result<Vec<String>>
where
    I: IntoIterator<Item = (&'a str, Option<&'a str>)>,
{
    // Two passes (unsets first, sets second) are part of the output
    // contract, so we collect once. The collected vector holds only
    // borrowed slices, so the per-entry cost is two pointers and an
    // `Option`-tag — much cheaper than cloning the source strings.
    let entries: Vec<(&str, Option<&str>)> = env.into_iter().collect();
    let mut parts: Vec<String> = Vec::with_capacity(entries.len() * 2);
    for (name, value) in &entries {
        if value.is_none() {
            parts.push("-u".to_string());
            parts.push((*name).to_string());
        }
    }
    for (name, value) in &entries {
        if let Some(v) = value {
            let qv = shlex::try_quote(v)
                .map_err(|_| Error::ArgumentContainsNul {
                    value: (*v).to_string(),
                })?
                .into_owned();
            parts.push(format!("{name}={qv}"));
        }
    }
    Ok(parts)
}

/// Adapt a slice of resolved [`EnvOp`]s to the `(name, value)` pair
/// shape consumed by [`env_tokens_quoted`].
fn env_ops_as_pairs(env_ops: &[EnvOp]) -> impl Iterator<Item = (&str, Option<&str>)> + '_ {
    env_ops.iter().map(|op| match op {
        EnvOp::Set { name, value } => (name.as_str(), Some(value.as_str())),
        EnvOp::Unset { name } => (name.as_str(), None),
    })
}

/// Adapt a slice of config-side [`EnvEntry`]s to the `(name, value)`
/// pair shape consumed by [`env_tokens_quoted`]. Used by `--list`
/// (`SPEC.md` §7.1) so both command-level and profile-level env
/// contributions can be rendered without rebuilding a `Vec<EnvOp>`.
fn env_entries_as_pairs(entries: &[EnvEntry]) -> impl Iterator<Item = (&str, Option<&str>)> + '_ {
    entries.iter().map(|e| match &e.value {
        EnvValue::Set(v) => (e.name.as_str(), Some(v.as_str())),
        EnvValue::Unset => (e.name.as_str(), None),
    })
}

/// Render just the argument tokens (no program, no pass-through),
/// shell-quoted and space-joined. Used by `--list` to render each
/// command's default args and per-profile args per `SPEC.md` §7.1.
///
/// Takes any borrowed iterator over [`Argument`] (slices, `Vec`
/// references, and `Vec<&Argument>` all satisfy it) so callers don't
/// need to materialize an owned vector to invoke the renderer.
///
/// # Errors
///
/// Returns [`Error::ArgumentContainsNul`] if any value contains a
/// NUL byte.
pub fn format_args<'a, I>(args: I) -> Result<String>
where
    I: IntoIterator<Item = &'a Argument>,
{
    shell_join(&args_to_tokens(args))
}

/// Render pass-through tokens as a single shell-quoted, space-joined
/// string. Used by `--explain` to surface the trailing CLI-supplied
/// passthrough block alongside the config-derived argv segments.
/// Returns an empty string for an empty input.
///
/// # Errors
///
/// Returns [`Error::PassthroughNotUtf8`] if any token is not valid
/// UTF-8, or [`Error::ArgumentContainsNul`] if any token contains a
/// NUL byte (which `shlex` cannot quote).
pub fn format_passthrough(passthrough: &[OsString]) -> Result<String> {
    let mut tokens: Vec<String> = Vec::with_capacity(passthrough.len());
    for pt in passthrough {
        let s = pt
            .to_str()
            .ok_or_else(|| Error::PassthroughNotUtf8 {
                lossy: pt.to_string_lossy().into_owned(),
            })?
            .to_string();
        tokens.push(s);
    }
    shell_join(&tokens)
}

/// Render config-side env entries as a `--list` line:
/// `-u UNSET NAME=value`, space-joined. Used by `--list` per
/// `SPEC.md` §7.1 to render both the command-level and per-profile
/// env channels straight from the parsed config. Returns an empty
/// string for an empty input. Values that need shell-quoting are
/// quoted; names are not (they're POSIX-portable per §2.9).
///
/// # Errors
///
/// Returns [`Error::ArgumentContainsNul`] if any value contains a
/// NUL byte.
pub fn format_env_entries(entries: &[EnvEntry]) -> Result<String> {
    Ok(env_tokens_quoted(env_entries_as_pairs(entries))?.join(" "))
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

    fn env_entry(name: &str, value: EnvValue) -> EnvEntry {
        EnvEntry {
            name: name.to_string(),
            name_span: SourceSpan::from((0, 0)),
            value,
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

    // --- format_passthrough (used by --explain to surface the
    //     trailing CLI-supplied tokens) ---

    #[test]
    fn format_passthrough_empty_returns_empty_string() {
        assert_eq!(format_passthrough(&[]).unwrap(), "");
    }

    #[test]
    fn format_passthrough_joins_plain_tokens_with_spaces() {
        let tokens = [OsString::from("-a"), OsString::from("--test")];
        assert_eq!(format_passthrough(&tokens).unwrap(), "-a --test");
    }

    #[test]
    fn format_passthrough_quotes_tokens_with_spaces() {
        let tokens = [OsString::from("a b"), OsString::from("c")];
        assert_eq!(format_passthrough(&tokens).unwrap(), "'a b' c");
    }

    #[cfg(unix)]
    #[test]
    fn format_passthrough_errors_on_non_utf8_token() {
        use std::os::unix::ffi::OsStringExt;
        let invalid = OsString::from_vec(vec![0xff, 0xfe]);
        let err = format_passthrough(&[invalid]).unwrap_err();
        assert!(matches!(err, Error::PassthroughNotUtf8 { .. }));
    }

    #[test]
    fn format_passthrough_errors_on_nul_byte() {
        let tokens = [OsString::from("a\0b")];
        let err = format_passthrough(&tokens).unwrap_err();
        assert!(matches!(err, Error::ArgumentContainsNul { .. }));
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
    fn format_env_entries_drops_leading_env_marker() {
        // For --list, the `env:` label conveys what `env(1)` would,
        // so format_env_entries strips the literal `env` prefix token.
        let entries = vec![
            env_entry("OLD", EnvValue::Unset),
            env_entry("A", EnvValue::Set("1".into())),
        ];
        assert_eq!(format_env_entries(&entries).unwrap(), "-u OLD A=1");
    }

    #[test]
    fn format_env_entries_empty_for_no_entries() {
        assert_eq!(format_env_entries(&[]).unwrap(), "");
    }

    #[test]
    fn format_env_entries_quotes_set_value_with_spaces() {
        let entries = vec![env_entry("MSG", EnvValue::Set("two words".into()))];
        assert_eq!(format_env_entries(&entries).unwrap(), "MSG='two words'");
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
