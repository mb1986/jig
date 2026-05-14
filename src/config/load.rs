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

/// Load and parse the config, returning the parsed [`Config`]
/// alongside a [`NamedSource`] that the validator can attach to
/// any constraint diagnostics.
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
pub fn load(explicit: Option<&Path>) -> Result<(Config, NamedSource<String>)> {
    let path = match explicit {
        Some(p) => p.to_path_buf(),
        None => discover_upward()?,
    };
    let content = fs::read_to_string(&path).map_err(|source| Error::ConfigIo {
        path: path.clone(),
        source,
    })?;
    let display_name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let config = parse::parse_str(&content, &display_name)?;
    let src = NamedSource::new(display_name, content);
    Ok((config, src))
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

        if let Some(h) = home.as_deref() {
            if dir == h {
                break;
            }
        }
    }

    Err(Error::ConfigNotFound {
        searched: format!("{PRIMARY}, {FALLBACK}"),
        from: cwd,
        up_to: last_checked,
    })
}
