//! Discover and read the config file, then hand off to the parser.
//!
//! Implements `SPEC.md` §2.1: when no explicit `--config` path is
//! given, start in the current working directory and walk upward
//! through ancestor directories, checking each for `jig.kdl` then
//! `.jig.kdl`. The first match wins; within one directory the
//! visible name beats the hidden one. The walk stops after checking
//! `$HOME` if it appears in the ancestor chain, otherwise it
//! continues to the filesystem root.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use miette::NamedSource;

use super::{Config, parse};
use crate::errors::{Error, Result};

const PRIMARY: &str = "jig.kdl";
const FALLBACK: &str = ".jig.kdl";

/// A loaded config file: the parsed [`Config`], a [`NamedSource`]
/// the validator attaches to any constraint diagnostics, and the
/// directory that contains the file (used as the anchor for
/// relative `cwd=` values per `SPEC.md` §2.12).
#[derive(Debug)]
pub struct Loaded {
    /// The parsed and structurally-valid configuration.
    pub config: Config,
    /// The KDL source text + file name, threaded into miette for
    /// span-aware diagnostics.
    pub src: NamedSource<String>,
    /// The directory containing the loaded config file. Relative
    /// paths in `cwd=` (`SPEC.md` §2.12) resolve against this.
    pub config_dir: PathBuf,
}

/// Locate and read the config file without parsing it.
///
/// Same discovery rules as [`load`] (explicit `--config` path wins,
/// otherwise upward walk from CWD bounded by `$HOME`), but stops
/// after reading the file's bytes. Used by paths that need the
/// raw text even when the file would not parse as KDL (currently
/// `--cat`).
///
/// # Errors
///
/// Returns [`Error::ConfigNotFound`] if no config file exists in
/// the search range, or [`Error::ConfigIo`] if reading the chosen
/// file fails.
pub fn locate_and_read(explicit: Option<&Path>) -> Result<(PathBuf, String)> {
    let path = match explicit {
        Some(p) => p.to_path_buf(),
        None => discover_upward()?,
    };
    let content = fs::read_to_string(&path).map_err(|source| Error::ConfigIo {
        path: path.clone(),
        source,
    })?;
    Ok((path, content))
}

/// Load and parse the config.
///
/// If `explicit` is `Some`, that path is used directly (relative
/// paths are resolved against the CWD as usual). Otherwise, the
/// search starts in the CWD and walks upward through ancestors,
/// checking [`PRIMARY`] then [`FALLBACK`] in each directory; the
/// walk stops after `$HOME` if encountered, or at the filesystem
/// root.
///
/// # Errors
///
/// Returns [`Error::ConfigNotFound`] if no config file exists in
/// the search range, [`Error::ConfigIo`] if reading the chosen
/// file fails, or any error produced by [`parse::parse_str`].
pub fn load(explicit: Option<&Path>) -> Result<Loaded> {
    let (path, content) = locate_and_read(explicit)?;
    let display_name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let config = parse::parse_str(&content, &display_name)?;
    let src = NamedSource::new(display_name, content);
    // Anchor for relative `cwd=` per `SPEC.md` §2.12.2. The upward
    // walk always produces an absolute path; an explicit `--config`
    // may be relative (including a bare filename like `jig.kdl`,
    // whose parent is `""`). We canonicalise to "absolute path of
    // the parent directory at jig-invocation time" so the SPEC §7.2
    // guarantee of an absolute path inside `(cd … && …)` holds.
    // Symlinks in the parent are preserved (no `realpath` resolution
    // — `env::current_dir()` returns the logical path).
    let config_dir = absolutise_parent(&path)?;
    Ok(Loaded {
        config,
        src,
        config_dir,
    })
}

/// Return the absolute directory containing `path`. If `path`'s
/// parent is already absolute, returns it as-is. Otherwise prepends
/// the current working directory: an empty parent (bare filename
/// like `jig.kdl`) and a relative parent (`./foo/jig.kdl`) both
/// resolve relative to the process CWD.
///
/// Note: a path whose `.parent()` is `None` (only the filesystem
/// root, which cannot be a config file) also falls through the
/// relative branch and joins onto CWD. That case is unreachable in
/// practice — `open(2)` on the root directory would have failed
/// earlier.
///
/// Errors only if `path` is relative and `env::current_dir()` itself
/// fails (a rare OS-level failure that already short-circuits the
/// discovery path in [`discover_upward`]).
fn absolutise_parent(path: &Path) -> Result<PathBuf> {
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    if parent.is_absolute() {
        return Ok(parent.to_path_buf());
    }
    let cwd = env::current_dir().map_err(|source| Error::ConfigIo {
        path: PathBuf::from("."),
        source,
    })?;
    if parent.as_os_str().is_empty() {
        Ok(cwd)
    } else {
        Ok(cwd.join(parent))
    }
}

fn discover_upward() -> Result<PathBuf> {
    let cwd = env::current_dir().map_err(|source| Error::ConfigIo {
        path: PathBuf::from("."),
        source,
    })?;
    let home = env::var_os("HOME").map(PathBuf::from);

    let mut last_checked = cwd.clone();
    for dir in cwd.ancestors() {
        last_checked = dir.to_path_buf();

        let primary = dir.join(PRIMARY);
        if primary.is_file() {
            return Ok(primary);
        }
        let fallback = dir.join(FALLBACK);
        if fallback.is_file() {
            return Ok(fallback);
        }

        if let Some(h) = home.as_deref()
            && dir == h
        {
            break;
        }
    }

    Err(Error::ConfigNotFound {
        searched: format!("{PRIMARY}, {FALLBACK}"),
        from: cwd,
        up_to: last_checked,
    })
}
