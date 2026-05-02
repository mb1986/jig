//! Discover and read the config file, then hand off to the parser.
//!
//! Implements `SPEC.md` §2.1: when no explicit `--config` path is
//! given, look for `./jig.kdl` then `./.jig.kdl` in the current
//! working directory; if both exist, the visible name wins.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use super::{Config, parse};
use crate::errors::{Error, Result};

const PRIMARY: &str = "jig.kdl";
const FALLBACK: &str = ".jig.kdl";

/// Load and parse the config.
///
/// If `explicit` is `Some`, that path is used directly (relative
/// paths are resolved against the CWD as usual). Otherwise, the
/// CWD is searched for [`PRIMARY`] then [`FALLBACK`].
///
/// # Errors
///
/// Returns [`Error::ConfigNotFound`] if no config file exists in
/// the search location, [`Error::ConfigIo`] if reading the chosen
/// file fails, or any error produced by [`parse::parse_str`].
pub fn load(explicit: Option<&Path>) -> Result<Config> {
    let path = match explicit {
        Some(p) => p.to_path_buf(),
        None => discover_in_cwd()?,
    };
    let content = fs::read_to_string(&path).map_err(|source| Error::ConfigIo {
        path: path.clone(),
        source,
    })?;
    let display_name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    parse::parse_str(&content, &display_name)
}

fn discover_in_cwd() -> Result<PathBuf> {
    let cwd = env::current_dir().map_err(|source| Error::ConfigIo {
        path: PathBuf::from("."),
        source,
    })?;
    let primary = cwd.join(PRIMARY);
    if primary.is_file() {
        return Ok(primary);
    }
    let fallback = cwd.join(FALLBACK);
    if fallback.is_file() {
        return Ok(fallback);
    }
    Err(Error::ConfigNotFound {
        searched: format!("./{PRIMARY}, ./{FALLBACK}"),
        cwd,
    })
}
