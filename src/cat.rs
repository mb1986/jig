//! `--cat` rendering: dump the loaded config file's raw contents
//! preceded by a `cat <path>` header.
//!
//! The header line looks like a shell `cat` invocation against the
//! resolved config path (cwd-relative when possible, absolute
//! otherwise, shell-quoted so paths with spaces still parse). It
//! is written to **stderr**, in bold when stderr is a terminal and
//! `NO_COLOR` is unset, plain text otherwise — matching the
//! pre-exec preview line in `SPEC.md` §3.4.1 so the two visual
//! cues read alike. Keeping the header off stdout means
//! `jig --cat | grep …` works cleanly against the file's content.
//!
//! The body is the file's bytes as read, written to stdout; no
//! re-encoding, no comment-stripping. `--cat` does not require the
//! file to parse as KDL, so users can dump a broken config while
//! debugging.

use std::io::{IsTerminal, Write};
use std::path::Path;

use crate::errors::{Error, Result};
use crate::path::render_config_path;

/// Write the `cat <path>` header to stderr and `contents` to
/// stdout, per the `--cat` design.
///
/// `path` is the canonical (typically absolute) path of the loaded
/// config file; it is rendered cwd-relative when possible via
/// [`render_config_path`]. `contents` is written verbatim — no
/// trailing newline is added if it is missing in the source.
///
/// # Errors
///
/// Returns [`Error::ArgumentContainsNul`] if the rendered path
/// contains a NUL byte and so cannot be shell-quoted. In practice
/// unreachable: a Unix filesystem path cannot contain NUL.
pub fn print(path: &Path, contents: &str) -> Result<()> {
    let rendered = render_config_path(path);
    let mut stderr = std::io::stderr().lock();
    let use_bold = stderr.is_terminal() && std::env::var_os("NO_COLOR").is_none();
    let header = format_header(&rendered, use_bold)?;
    let _ = writeln!(stderr, "{header}");
    let _ = std::io::stdout().lock().write_all(contents.as_bytes());
    Ok(())
}

/// Build the `cat <path>` header line. `rendered_path` is the display
/// form (cwd-relative when possible, absolute otherwise); it is
/// shell-quoted so paths with spaces or other shell metachars still
/// parse as a single argument. When `use_bold` is set the line is
/// wrapped in ANSI bold escapes (`\x1b[1m…\x1b[0m`).
fn format_header(rendered_path: &str, use_bold: bool) -> Result<String> {
    let quoted = shlex::try_quote(rendered_path).map_err(|_| Error::ArgumentContainsNul {
        value: rendered_path.to_string(),
    })?;
    let line = format!("cat {quoted}");
    Ok(if use_bold {
        format!("\x1b[1m{line}\x1b[0m")
    } else {
        line
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_plain_renders_unquoted_path_as_is() {
        let h = format_header("jig.kdl", false).unwrap();
        assert_eq!(h, "cat jig.kdl");
    }

    #[test]
    fn header_plain_shell_quotes_path_with_spaces() {
        // shlex single-quotes any token with shell-significant chars.
        let h = format_header("../my dir/jig.kdl", false).unwrap();
        assert_eq!(h, "cat '../my dir/jig.kdl'");
    }

    #[test]
    fn header_bold_wraps_line_in_ansi_escapes() {
        let h = format_header("jig.kdl", true).unwrap();
        assert_eq!(h, "\x1b[1mcat jig.kdl\x1b[0m");
    }

    #[test]
    fn header_rejects_path_with_nul_byte() {
        // `shlex::try_quote` returns Err for embedded NULs; we surface
        // that as `ArgumentContainsNul` rather than a silent fallback.
        let err = format_header("jig\u{0}.kdl", false).unwrap_err();
        assert!(matches!(err, Error::ArgumentContainsNul { .. }));
    }
}
