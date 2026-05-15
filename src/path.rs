//! Path-rendering helpers shared by `--list` and `--explain`.
//!
//! Both surfaces print the loaded config-file path at the top of
//! their output and want the same display rules: prefer a cwd-relative
//! rendering (with `..` segments when needed), fall back to the
//! absolute path when no common ancestor exists or the current
//! directory can't be determined.

use std::path::{Component, Path, PathBuf};

/// Render a config-file path for display — try relative-to-cwd first
/// (with `..` segments where needed); fall back to the absolute path
/// if the current directory can't be determined.
///
/// In production both inputs are absolute (callers absolutise the
/// config path at load time), so [`diff_paths`] always returns
/// `Some(..)` and the `unwrap_or(abs)` branch is defensive — kept to
/// keep the function total for callers that might pass a relative
/// path in tests or future code paths.
#[must_use]
pub fn render_config_path(path: &Path) -> String {
    let Ok(cwd) = std::env::current_dir() else {
        return path.display().to_string();
    };
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    diff_paths(&abs, &cwd).unwrap_or(abs).display().to_string()
}

/// Compute a relative path from `base` to `path`, returning `None`
/// when the two have incompatible prefixes (one absolute, one
/// relative). Mirrors the standard "pathdiff" algorithm: walk both
/// component lists in lockstep, emit one `..` per remaining base
/// component once they diverge, then append the rest of `path`.
#[must_use]
pub fn diff_paths(path: &Path, base: &Path) -> Option<PathBuf> {
    if path.is_absolute() != base.is_absolute() {
        return None;
    }
    let mut ita = path.components();
    let mut itb = base.components();
    let mut comps: Vec<Component<'_>> = Vec::new();
    loop {
        match (ita.next(), itb.next()) {
            (None, None) => break,
            (Some(a), None) => {
                comps.push(a);
                comps.extend(ita.by_ref());
                break;
            }
            (None, _) => comps.push(Component::ParentDir),
            (Some(a), Some(b)) if comps.is_empty() && a == b => {}
            (Some(a), Some(Component::CurDir)) => comps.push(a),
            (Some(_), Some(Component::ParentDir)) => return None,
            (Some(a), Some(_)) => {
                comps.push(Component::ParentDir);
                for _ in itb.by_ref() {
                    comps.push(Component::ParentDir);
                }
                comps.push(a);
                comps.extend(ita.by_ref());
                break;
            }
        }
    }
    if comps.is_empty() {
        // path == base — should not occur for a config-file path,
        // but keep the path display non-empty just in case.
        Some(PathBuf::from("."))
    } else {
        Some(comps.iter().map(|c| c.as_os_str()).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_paths_subdir_to_cwd() {
        // Config sits inside cwd → relative subpath, no `..`.
        let d = diff_paths(
            Path::new("/home/me/proj/jig.kdl"),
            Path::new("/home/me/proj"),
        );
        assert_eq!(d, Some(PathBuf::from("jig.kdl")));
    }

    #[test]
    fn diff_paths_nested_subdir() {
        let d = diff_paths(
            Path::new("/home/me/proj/sub/dir/jig.kdl"),
            Path::new("/home/me/proj"),
        );
        assert_eq!(d, Some(PathBuf::from("sub/dir/jig.kdl")));
    }

    #[test]
    fn diff_paths_parent_dir_uses_dotdot() {
        // cwd is a subdir; config sits in an ancestor → `..` segment.
        let d = diff_paths(
            Path::new("/home/me/proj/jig.kdl"),
            Path::new("/home/me/proj/sub"),
        );
        assert_eq!(d, Some(PathBuf::from("../jig.kdl")));
    }

    #[test]
    fn diff_paths_grandparent_dir_uses_two_dotdots() {
        let d = diff_paths(
            Path::new("/home/me/proj/jig.kdl"),
            Path::new("/home/me/proj/sub/deeper"),
        );
        assert_eq!(d, Some(PathBuf::from("../../jig.kdl")));
    }

    #[test]
    fn diff_paths_returns_none_when_one_relative_one_absolute() {
        let d = diff_paths(Path::new("/abs/jig.kdl"), Path::new("rel"));
        assert_eq!(d, None);
    }

    #[test]
    fn diff_paths_returns_dot_when_path_equals_base() {
        let d = diff_paths(Path::new("/home/me/proj"), Path::new("/home/me/proj"));
        assert_eq!(d, Some(PathBuf::from(".")));
    }
}
