//! Spawn the resolved command, wait, and translate the child's
//! exit status into the wrapper-tool exit code per `SPEC.md` §3.5
//! and §3.6.
//!
//! Behavior:
//!
//! - Standard streams (stdin / stdout / stderr) are inherited.
//!   `std::process::Command::status` does this by default.
//! - Signals delivered to `jig` (SIGINT / SIGTERM) reach the child
//!   naturally because the child is in the same process group;
//!   no explicit forwarding is performed.
//! - On Unix, a child killed by signal `N` exits with `128 + N`,
//!   following the shell convention. Per Q9 v1 is Unix-only;
//!   non-Unix platforms fall back to whatever `ExitStatus::code()`
//!   returns (or 125 if neither is available).
//! - `io::ErrorKind::NotFound` from the spawn maps to
//!   [`Error::ExecNotFound`] (exit 127); `PermissionDenied` maps
//!   to [`Error::ExecNotExecutable`] (exit 126); anything else
//!   maps to [`Error::ExecSpawnFailed`] (exit 125).

use std::ffi::{OsStr, OsString};
use std::io;
use std::process::{Command, ExitStatus};

use crate::errors::{Error, Result};

/// Spawn `program` with `args` (inheriting stdio), wait, and
/// return the exit code to propagate to the caller of `jig`.
///
/// # Errors
///
/// Returns [`Error::ExecNotFound`] if `program` cannot be located,
/// [`Error::ExecNotExecutable`] if it is found but not executable,
/// or [`Error::ExecSpawnFailed`] for any other I/O failure during
/// spawn.
pub fn run(program: &str, args: &[OsString]) -> Result<i32> {
    let status = Command::new(OsStr::new(program)).args(args).status();
    match status {
        Ok(s) => Ok(extract_exit_code(s)),
        Err(e) => Err(map_spawn_error(program, e)),
    }
}

fn map_spawn_error(program: &str, source: io::Error) -> Error {
    match source.kind() {
        io::ErrorKind::NotFound => Error::ExecNotFound {
            program: program.to_string(),
        },
        io::ErrorKind::PermissionDenied => Error::ExecNotExecutable {
            program: program.to_string(),
            source,
        },
        _ => Error::ExecSpawnFailed {
            program: program.to_string(),
            source,
        },
    }
}

#[cfg(unix)]
fn extract_exit_code(status: ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    // No `code()` on Unix means the child was signal-killed.
    // Fall back to 125 if both accessors return `None`, which
    // should not happen on Unix in practice.
    status
        .code()
        .unwrap_or_else(|| status.signal().map_or(125, |sig| 128 + sig))
}

#[cfg(not(unix))]
fn extract_exit_code(status: ExitStatus) -> i32 {
    status.code().unwrap_or(125)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_not_found_to_exec_not_found() {
        let e = map_spawn_error("missing", io::Error::from(io::ErrorKind::NotFound));
        assert!(matches!(e, Error::ExecNotFound { ref program } if program == "missing"));
    }

    #[test]
    fn maps_permission_denied_to_not_executable() {
        let e = map_spawn_error("bad", io::Error::from(io::ErrorKind::PermissionDenied));
        assert!(matches!(e, Error::ExecNotExecutable { ref program, .. } if program == "bad"));
    }

    #[test]
    fn maps_other_to_spawn_failed() {
        let e = map_spawn_error("weird", io::Error::from(io::ErrorKind::Other));
        assert!(matches!(e, Error::ExecSpawnFailed { ref program, .. } if program == "weird"));
    }
}
