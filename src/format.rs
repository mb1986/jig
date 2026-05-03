//! Convert a resolved argument list into the single shell-quoted
//! `--dry-run` line per `SPEC.md` §7.2.
//!
//! Implements `SPEC.md` §2.5 (key prefix synthesis), §2.4.1 (`#true`
//! → bare flag, `#false` → suppressed at the resolve layer), §3.3
//! (pass-through trails the resolved args), and §7.2 (single-line
//! shell-quoted dry-run output).
//!
//! `to_argv` (the `OsString` form for `Command::args`) lands in
//! Step 6 alongside `exec.rs`, which is the only consumer.

use std::ffi::OsString;

use crate::config::{Argument, FlagValue};
use crate::errors::{Error, Result};

/// Build the single shell-quoted line emitted by `--dry-run`. Per
/// `SPEC.md` §7.2 the output must be copy-pasteable into a POSIX
/// shell. Per Q7, non-UTF-8 pass-through arguments error rather
/// than lossy-render.
///
/// # Errors
///
/// Returns [`Error::PassthroughNotUtf8`] if a pass-through argument
/// is not valid UTF-8, or [`Error::ArgumentContainsNul`] if any
/// value contains a NUL byte (which `shlex` cannot quote).
pub fn to_dry_run(program: &str, args: &[Argument], passthrough: &[OsString]) -> Result<String> {
    let mut tokens: Vec<String> = Vec::with_capacity(args.len() * 2 + passthrough.len() + 1);
    tokens.push(program.to_string());
    for arg in args {
        match arg {
            Argument::Flag {
                key,
                value: FlagValue::Bool(true),
                ..
            } => tokens.push(key.to_cli_flag()),
            Argument::Flag {
                value: FlagValue::Bool(false),
                ..
            } => {
                // Resolve already drops these. Defensive no-op.
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
    for pt in passthrough {
        let s = pt
            .to_str()
            .ok_or_else(|| Error::PassthroughNotUtf8 {
                lossy: pt.to_string_lossy().into_owned(),
            })?
            .to_string();
        tokens.push(s);
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FlagKey, FlagValue};
    use miette::SourceSpan;

    fn flag(key: FlagKey, value: FlagValue) -> Argument {
        Argument::Flag {
            key,
            key_span: SourceSpan::from((0, 0)),
            value,
        }
    }

    #[test]
    fn dry_run_basic() {
        let args = vec![flag(
            FlagKey::Inferred("host".into()),
            FlagValue::Literal("0.0.0.0".into()),
        )];
        let s = to_dry_run("llama-server", &args, &[]).unwrap();
        assert_eq!(s, "llama-server --host 0.0.0.0");
    }

    #[test]
    fn dry_run_quotes_spaces() {
        let args = vec![Argument::Positional("/path with spaces/file".into())];
        let s = to_dry_run("foo", &args, &[]).unwrap();
        // shlex picks single-quoting for values with shell-significant chars
        assert!(s.contains("'/path with spaces/file'"));
    }

    #[test]
    fn dry_run_quotes_glob_chars() {
        let args = vec![Argument::Positional("*.txt".into())];
        let s = to_dry_run("foo", &args, &[]).unwrap();
        assert!(s.contains("'*.txt'"));
    }

    #[test]
    fn dry_run_omits_value_for_bool_true() {
        let args = vec![flag(
            FlagKey::Inferred("flash-attn".into()),
            FlagValue::Bool(true),
        )];
        let s = to_dry_run("foo", &args, &[]).unwrap();
        assert_eq!(s, "foo --flash-attn");
    }

    #[test]
    fn dry_run_passthrough_quotes_when_needed() {
        let s = to_dry_run("foo", &[], &[OsString::from("a b")]).unwrap();
        assert_eq!(s, "foo 'a b'");
    }

    #[cfg(unix)]
    #[test]
    fn dry_run_errors_on_non_utf8_passthrough() {
        use std::os::unix::ffi::OsStringExt;
        let invalid = OsString::from_vec(vec![0xff, 0xfe]);
        let err = to_dry_run("foo", &[], &[invalid]).unwrap_err();
        assert!(matches!(err, Error::PassthroughNotUtf8 { .. }));
    }
}
