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
use std::path::Path;
use std::process::{Command, ExitStatus};

use crate::errors::{Error, Result};
use crate::resolve::EnvOp;

/// Spawn `program` with `args` (inheriting stdio), apply `env_ops`
/// to the child's environment per `SPEC.md` §3.6, wait, and return
/// the exit code to propagate to the caller of `jig`.
///
/// `env_ops` is a list of [`EnvOp::Set`] / [`EnvOp::Unset`] entries
/// that layer on top of the inherited environment; `jig` never
/// wholesale-clears the env (`Command::env_clear`).
///
/// If `cwd` is `Some`, the child is spawned with that working
/// directory (`Command::current_dir`) per `SPEC.md` §2.12 / §3.6.
/// A `NotFound` / `NotADirectory` / `PermissionDenied` failure here
/// is the signature of a bad `cwd=` value (the program's own lookup
/// has not yet happened), so we pre-check the directory's existence
/// and surface [`Error::CwdNotUsable`] (exit 125) — distinct from
/// the program-not-found case.
///
/// # Errors
///
/// Returns [`Error::CwdContainsNul`] if `cwd` contains a NUL byte,
/// [`Error::CwdNotUsable`] if `cwd` is supplied but is otherwise
/// not a usable directory, [`Error::ExecNotFound`] if `program`
/// cannot be located, [`Error::ExecNotExecutable`] if it is found
/// but not executable, or [`Error::ExecSpawnFailed`] for any other
/// I/O failure during spawn.
pub fn run(program: &str, args: &[OsString], env_ops: &[EnvOp], cwd: Option<&Path>) -> Result<i32> {
    // Embedded NUL byte in the cwd: a Unix filesystem path cannot
    // contain a NUL, but KDL `\u{0}` escapes can put one into a
    // `cwd=` string. Detect it explicitly so the diagnostic names
    // the cause precisely, mirroring the dry-run path
    // (`crate::format::to_dry_run`).
    if let Some(dir) = cwd
        && dir.as_os_str().as_encoded_bytes().contains(&0)
    {
        return Err(Error::CwdContainsNul {
            path: dir.to_string_lossy().into_owned(),
        });
    }

    // Pre-check the cwd so a bad path produces `CwdNotUsable` (exit
    // 125) rather than the spawn-time `NotFound` that would
    // otherwise look like a missing program (exit 127). The check
    // is intentionally minimal — `is_dir()` is sufficient and avoids
    // racing against the spawn for permission/traversal failures
    // (those still come back from spawn and are also mapped to
    // `CwdNotUsable` below).
    if let Some(dir) = cwd
        && !dir.is_dir()
    {
        let source = io::Error::from(io::ErrorKind::NotFound);
        return Err(Error::CwdNotUsable {
            path: dir.to_path_buf(),
            source,
        });
    }

    let mut cmd = Command::new(OsStr::new(program));
    cmd.args(args);
    for op in env_ops {
        match op {
            EnvOp::Set { name, value } => {
                cmd.env(name, value);
            }
            EnvOp::Unset { name } => {
                cmd.env_remove(name);
            }
        }
    }
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let status = cmd.status();
    match status {
        Ok(s) => Ok(extract_exit_code(s)),
        Err(e) => Err(map_spawn_error(program, e, cwd)),
    }
}

fn map_spawn_error(program: &str, source: io::Error, cwd: Option<&Path>) -> Error {
    // When `current_dir` is set, the kernel's `chdir` happens before
    // `execvp`, so `NotFound` / `PermissionDenied` / `NotADirectory`
    // can equally come from a bad cwd or a bad program. Disambiguate:
    //
    // - If the cwd's `is_dir()` is now false (vanished, never was, or
    //   is not a directory), blame the cwd. This is a belt-and-braces
    //   re-check of the pre-spawn guard in `run`: the only way to get
    //   here is a TOCTOU race where the directory was removed between
    //   the pre-check and the spawn. Keeping the second check means
    //   the diagnostic is still correct in that race.
    // - Else if the error is `PermissionDenied` and `canonicalize`
    //   on the cwd fails, the directory is present but not
    //   traversable (e.g. `0700` owned by another user, or missing
    //   `+x` on a parent component) — blame the cwd. `canonicalize`
    //   walks every ancestor with `+x` required, which is exactly
    //   what `chdir(2)` needs.
    // - Otherwise the failure is about the program.
    if let Some(dir) = cwd {
        if !dir.is_dir() {
            return Error::CwdNotUsable {
                path: dir.to_path_buf(),
                source,
            };
        }
        if source.kind() == io::ErrorKind::PermissionDenied && std::fs::canonicalize(dir).is_err() {
            return Error::CwdNotUsable {
                path: dir.to_path_buf(),
                source,
            };
        }
    }
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
        let e = map_spawn_error("missing", io::Error::from(io::ErrorKind::NotFound), None);
        assert!(matches!(e, Error::ExecNotFound { ref program } if program == "missing"));
    }

    #[test]
    fn maps_permission_denied_to_not_executable() {
        let e = map_spawn_error(
            "bad",
            io::Error::from(io::ErrorKind::PermissionDenied),
            None,
        );
        assert!(matches!(e, Error::ExecNotExecutable { ref program, .. } if program == "bad"));
    }

    #[test]
    fn maps_other_to_spawn_failed() {
        let e = map_spawn_error("weird", io::Error::from(io::ErrorKind::Other), None);
        assert!(matches!(e, Error::ExecSpawnFailed { ref program, .. } if program == "weird"));
    }

    #[test]
    fn missing_cwd_routed_to_cwd_not_usable() {
        // Pretend the spawn returned NotFound, but the cwd we supplied
        // does not exist. `map_spawn_error` should attribute the
        // failure to the cwd, not the program.
        let bad = Path::new("/nonexistent-dir-for-jig-tests");
        let e = map_spawn_error("ls", io::Error::from(io::ErrorKind::NotFound), Some(bad));
        assert!(matches!(e, Error::CwdNotUsable { ref path, .. } if path == bad));
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_cwd_with_permission_denied_routed_to_cwd_not_usable() {
        // The `is_dir()` fast-path won't fire (the dir exists and is
        // a directory), but `canonicalize()` does — it requires `+x`
        // on every component, which a 0o000 directory denies. The
        // disambiguation should attribute the failure to the cwd.
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let locked = tmp.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Precondition: chmod 0 must actually block this process from
        // canonicalising the path. As root, mode bits don't block, in
        // which case the test scenario is unreachable — skip rather
        // than fail spuriously. Restore permissions so tempdir
        // cleanup works either way.
        if std::fs::canonicalize(&locked).is_ok() {
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
            return;
        }
        let e = map_spawn_error(
            "ls",
            io::Error::from(io::ErrorKind::PermissionDenied),
            Some(&locked),
        );
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            matches!(e, Error::CwdNotUsable { ref path, .. } if path == &locked),
            "expected CwdNotUsable, got {e:?}"
        );
    }

    #[test]
    fn permission_denied_without_cwd_still_blames_program() {
        // Sanity check: with no cwd supplied, PermissionDenied is
        // still attributed to the program (exit 126 path).
        let e = map_spawn_error(
            "bad",
            io::Error::from(io::ErrorKind::PermissionDenied),
            None,
        );
        assert!(matches!(e, Error::ExecNotExecutable { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn cwd_with_embedded_nul_is_cwd_contains_nul() {
        // The live-exec NUL guard short-circuits before is_dir(): a
        // path with an embedded NUL is a CwdContainsNul, not the
        // generic CwdNotUsable that an is_dir-false would produce.
        use std::os::unix::ffi::OsStrExt;
        let bad = std::ffi::OsStr::from_bytes(b"/tmp/a\0b");
        let bad_path = std::path::Path::new(bad);
        // The program here is irrelevant — `run` errors before spawn.
        let err = run("true", &[], &[], Some(bad_path)).unwrap_err();
        assert!(
            matches!(err, Error::CwdContainsNul { ref path } if path.contains('\0')),
            "expected CwdContainsNul, got {err:?}"
        );
    }
}
