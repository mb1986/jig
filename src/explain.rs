//! `--explain` rendering.
//!
//! Renders a [`ResolutionTrace`] (from
//! [`crate::resolve::resolve_with_trace`]) as a header plus an
//! annotated-argv body. Each emitted argv segment is numbered; below
//! the resolved line, one footnote per number lists the contributing
//! tier(s), source position(s), and any "lost-to-merge" facts that
//! the user wouldn't be able to infer from the resolved line alone.
//!
//! The negative-space rule (the "dropped" lines) is the only place
//! where the renderer applies its own filter: simple two-tier
//! single-mode overrides omit the dropped line because
//! "position from / value from" already implies the loser; `#false`
//! suppression and middle-tier losses in 3+ tier chains are
//! always surfaced because their absence would leave an argv slot
//! unexplained.
//!
//! Coloring reuses [`crate::theme::Theme`]: bold blue for the
//! command name, italic magenta for profile names, dim for labels
//! and source positions, green for `(alias: …)`, amber for
//! `(inherited)`, bright white for values, and dim+strikethrough
//! for dropped values.

use std::path::{Component, Path, PathBuf};

use miette::SourceSpan;

use crate::config::{Argument, EnvValue};
use crate::errors::Result;
use crate::format;
use crate::resolve::{
    Contributor, ContributorRole, CwdTrace, DroppedInfo, DroppedReason, EnvOp, EnvShadowed,
    EnvTrace, ResolutionTrace, Resolved, SegmentTrace, SuppressedKey,
};
use crate::theme::Theme;

/// Render `trace` (paired with `resolved`) to stdout. `source_name`
/// (typically the bare filename) appears next to each `loc()`
/// rendering; `source_path` (the canonical absolute path) is shown
/// once in the header, displayed relative to the current working
/// directory when possible. `source_bytes` is the loaded config
/// text, used to translate `SourceSpan` offsets to line numbers.
///
/// # Errors
///
/// Returns [`crate::errors::Error::ArgumentContainsNul`] if a value
/// contains a NUL byte (which `shlex` cannot quote in the resolved
/// line).
pub fn print(
    resolved: &Resolved,
    trace: &ResolutionTrace,
    source_name: &str,
    source_path: &Path,
    source_bytes: &str,
) -> Result<()> {
    let theme = Theme::from_stdout();
    // Pad every footnote marker to the width of the largest one
    // (e.g. `[1]` becomes ` [1]` when some segment is `[10]`, so the
    // closing `]` aligns vertically). The contributor indent below
    // each footnote tracks the marker width so the +1 visual nest
    // under segment text holds for any segment count.
    let marker_width = format!("[{}]", resolved.args.len()).chars().count();
    let contributor_indent = " ".repeat(marker_width + 5);
    let ctx = RenderCtx {
        theme,
        source_name,
        source_path,
        source_bytes,
        marker_width,
        contributor_indent,
    };

    print_header(resolved, trace, &ctx);
    println!();
    print_resolved_line(resolved, &ctx)?;

    for (k, (arg, seg)) in resolved.args.iter().zip(trace.segments.iter()).enumerate() {
        println!();
        print_footnote(k + 1, arg, seg, &ctx)?;
    }

    if !trace.env.is_empty() {
        println!();
        print_env_section(&trace.env, &ctx);
    }

    if !matches!(trace.cwd, CwdTrace::Inherited) {
        println!();
        print_cwd_section(&trace.cwd, &ctx);
    }

    if !trace.suppressed.is_empty() {
        println!();
        print_suppressed_section(&trace.suppressed, &ctx);
    }

    Ok(())
}

/// Bundle of context passed into every print helper. Saves shuffling
/// the same arguments between every function. `marker_width` and
/// `contributor_indent` are derived once per call so that single-
/// digit and double-digit footnote layouts use the same column
/// scheme.
struct RenderCtx<'a> {
    theme: Theme,
    source_name: &'a str,
    source_path: &'a Path,
    source_bytes: &'a str,
    marker_width: usize,
    contributor_indent: String,
}

impl RenderCtx<'_> {
    /// Format a span as `"<name>:<line>"` (1-based). Used everywhere a
    /// source position is displayed in `--explain` output.
    fn loc(&self, span: SourceSpan) -> String {
        let line = line_of(self.source_bytes, span);
        format!("{}:{}", self.source_name, line)
    }
}

/// Convert a `SourceSpan` offset to a 1-based line number by counting
/// newlines in `source` up to (and not including) the offset.
fn line_of(source: &str, span: SourceSpan) -> usize {
    let offset = span.offset().min(source.len());
    source[..offset].bytes().filter(|&b| b == b'\n').count() + 1
}

/// Render a config-file path for the `config:` header line — try
/// relative-to-cwd first (with `..` segments where needed); fall
/// back to the absolute path if the current directory can't be
/// determined or the two paths share no common ancestor.
fn render_config_path(path: &Path) -> String {
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
fn diff_paths(path: &Path, base: &Path) -> Option<PathBuf> {
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

// --- header ---

fn print_header(resolved: &Resolved, trace: &ResolutionTrace, ctx: &RenderCtx<'_>) {
    let t = ctx.theme;
    // `program:` line — name + optional alias annotation.
    let mut header = format!(
        "{}  {}",
        t.label("program: "),
        t.cmd_name(&resolved.program)
    );
    if let Some(alias) = &trace.alias {
        header.push_str("  ");
        header.push_str(&t.alias_annotation(&format!("(alias: {alias})")));
    }
    println!("{header}");

    // `config:` line — always shown, rendered relative to cwd when
    // possible. With cwd-relative rendering the user sees
    // `jig.kdl`, `subdir/jig.kdl`, or `../jig.kdl` depending on
    // where the upward search found the file.
    let config_display = render_config_path(ctx.source_path);
    println!("{}  {}", t.label("config:  "), t.value(&config_display));

    // `selected:` — only when a profile is selected.
    if let Some(p) = &trace.selected_profile {
        println!("{}  {}", t.label("selected:"), t.profile_name(p));
    }

    // `chain:` — only when there's actual inheritance to show
    // (length >= 2). A single-profile chain is just the leaf, which
    // the `selected:` line already names.
    if trace.chain.len() >= 2 {
        let names: Vec<String> = trace.chain.iter().map(|n| t.profile_name(n)).collect();
        println!("{}  {}", t.label("chain:   "), names.join(&t.label(" -> ")));
    }
}

// --- resolved line ---

/// Print the resolved argv on its own line, interleaving dim `[N]`
/// markers in front of each segment. Inline placement means markers
/// wrap with their segments — no alignment break when the resolved
/// line exceeds the terminal width. Each `[N]` matches the numbered
/// footnote that explains the segment.
fn print_resolved_line(resolved: &Resolved, ctx: &RenderCtx<'_>) -> Result<()> {
    let indent = "  ";
    println!("{}", ctx.theme.label("resolved:"));
    print!("{indent}{}", ctx.theme.cmd_name(&resolved.program));
    let mut n: usize = 0;
    for arg in &resolved.args {
        let text = format::format_args(std::iter::once(arg))?;
        if text.is_empty() {
            // Defensive: a `#false` / `#null` shouldn't have reached
            // Resolved::args, but skip empties just in case.
            continue;
        }
        n += 1;
        print!(
            " {} {}",
            ctx.theme.label(&format!("[{n}]")),
            ctx.theme.value(&text),
        );
    }
    println!();
    Ok(())
}

// --- per-segment footnote ---

fn print_footnote(
    n: usize,
    arg: &Argument,
    segment: &SegmentTrace,
    ctx: &RenderCtx<'_>,
) -> Result<()> {
    let t = ctx.theme;
    let segment_text = format::format_args(std::iter::once(arg))?;
    // Left-pad the marker so the closing `]` aligns at a fixed
    // column across all footnotes — `[1]` becomes ` [1]` and sits
    // flush right against `[10]`. ANSI styling wraps the padded form
    // so the leading spaces stay plain.
    let marker = format!("[{n}]");
    let width = ctx.marker_width;
    let padded_marker = format!("{marker:>width$}");
    let header_left = format!("  {}  {}", t.label(&padded_marker), t.value(&segment_text));
    match &segment.mode_summary {
        Some(summary) => println!("{header_left}      {}", t.label(summary)),
        None => println!("{header_left}"),
    }

    let multi = segment.contributors.len() > 1;
    for c in &segment.contributors {
        print_contributor(c, multi, ctx);
    }
    // The trace builder already applies variant C — only the
    // non-obvious drops (`#false` suppression, middle-tier loss in
    // 3+ tier chains) make it into `segment.dropped`. The renderer
    // surfaces every entry that's there.
    for d in &segment.dropped {
        print_dropped(d, ctx);
    }
    Ok(())
}

fn print_contributor(c: &Contributor, multi: bool, ctx: &RenderCtx<'_>) {
    let t = ctx.theme;
    let role = if multi {
        contributor_role_label(c.role)
    } else {
        String::new()
    };
    let inherited = if c.inherited {
        format!("  {}", t.extends_annotation("(inherited)"))
    } else {
        String::new()
    };
    // When a `#null` ghost wins the single-mode position, tell the
    // user — without this hint the line looks identical to a regular
    // default contributing position, which is misleading.
    let ghost = if c.ghost {
        format!("  {}", t.label("(#null ghost — no value)"))
    } else {
        String::new()
    };
    let tier_label = tier_label_styled(c.tier, &c.tier_label, t);
    let loc = c
        .span
        .map_or_else(String::new, |s| format!("  {}", t.label(&ctx.loc(s))));
    let indent = &ctx.contributor_indent;
    if role.is_empty() {
        println!("{indent}{tier_label}{loc}{inherited}{ghost}");
    } else {
        println!(
            "{indent}{}  {tier_label}{loc}{inherited}{ghost}",
            t.label(&role)
        );
    }
}

/// Map a `ContributorRole` to its left-column label, padded so the
/// `value from` / `position from` columns align within one footnote.
fn contributor_role_label(role: ContributorRole) -> String {
    match role {
        ContributorRole::Sole => "from         ".to_string(),
        ContributorRole::PositionOnly => "position from".to_string(),
        ContributorRole::ValueOnly => "value    from".to_string(),
        ContributorRole::Marker => "marker  from ".to_string(),
        ContributorRole::Repeat => "occurrence   ".to_string(),
    }
}

fn tier_label_styled(tier: usize, label: &str, theme: Theme) -> String {
    if tier == 0 {
        theme.label(label)
    } else {
        theme.profile_name(label)
    }
}

fn print_dropped(d: &DroppedInfo, ctx: &RenderCtx<'_>) {
    let t = ctx.theme;
    let role_tag = match &d.reason {
        DroppedReason::SuppressedByFalse {
            by_tier_label,
            by_span,
        } => format!("  (cleared by {} at {})", by_tier_label, ctx.loc(*by_span)),
        DroppedReason::SelfFalse => "  (suppressor)".to_string(),
        DroppedReason::MiddleTierLost => "  (lost to higher tier)".to_string(),
    };
    let indent = &ctx.contributor_indent;
    println!(
        "{indent}{}  {}  {}{}",
        tier_label_styled(d.tier, &d.tier_label, t),
        t.label(&ctx.loc(d.span)),
        t.dropped(&d.rendered_value),
        t.label(&role_tag),
    );
}

// --- env section ---

fn print_env_section(env: &[EnvTrace], ctx: &RenderCtx<'_>) {
    let t = ctx.theme;
    println!("{}", t.label("env:"));
    for entry in env {
        let summary = match &entry.outcome {
            EnvOp::Set { name, value } => format!("{name}={value}"),
            EnvOp::Unset { name } => format!("-u {name}"),
        };
        let inherited = if entry.winner_inherited {
            format!("  {}", t.extends_annotation("(inherited)"))
        } else {
            String::new()
        };
        println!("  {}", t.value(&summary));
        let indent = &ctx.contributor_indent;
        println!(
            "{indent}{}  {}{inherited}",
            tier_label_styled(entry.winner_tier, &entry.winner_tier_label, t),
            t.label(&ctx.loc(entry.winner_span)),
        );
        for s in &entry.shadowed {
            print_env_shadowed(s, ctx);
        }
    }
}

fn print_env_shadowed(s: &EnvShadowed, ctx: &RenderCtx<'_>) {
    let t = ctx.theme;
    let rendered = match &s.value {
        EnvValue::Set(v) => format!("\"{v}\""),
        EnvValue::Unset => "#false".to_string(),
    };
    let indent = &ctx.contributor_indent;
    println!(
        "{indent}{}  {}  {}{}",
        tier_label_styled(s.tier, &s.tier_label, t),
        t.label(&ctx.loc(s.span)),
        t.dropped(&rendered),
        t.label("  (shadowed)"),
    );
}

// --- cwd section ---

fn print_cwd_section(cwd: &CwdTrace, ctx: &RenderCtx<'_>) {
    let t = ctx.theme;
    println!("{}", t.label("cwd:"));
    let CwdTrace::Resolved {
        source,
        resolved,
        tier,
        tier_label,
        span,
        inherited,
    } = cwd
    else {
        // Inherited case already filtered out by the caller.
        return;
    };
    println!("  {}", t.value(&resolved.display().to_string()));
    let inherited_tag = if *inherited {
        format!("  {}", t.extends_annotation("(inherited)"))
    } else {
        String::new()
    };
    // Only show the original source-text when it differs from the
    // resolved absolute path — for an absolute `cwd="/srv"` the two
    // are identical and the parenthetical would be noise.
    let source_tag = if Path::new(source) == resolved.as_path() {
        String::new()
    } else {
        format!("  {}", t.label(&format!("(source: {source:?})")))
    };
    let indent = &ctx.contributor_indent;
    println!(
        "{indent}{}  {}{source_tag}{inherited_tag}",
        tier_label_styled(*tier, tier_label, t),
        t.label(&ctx.loc(*span)),
    );
}

// --- suppressed-keys section ---

fn print_suppressed_section(suppressed: &[SuppressedKey], ctx: &RenderCtx<'_>) {
    let t = ctx.theme;
    println!("{}", t.label("suppressed:"));
    for s in suppressed {
        println!("  {}", t.value(&s.key));
        // Each entry's role-tag (`(cleared)` / `(suppressor)`)
        // makes the cause obvious without an extra summary line.
        for entry in &s.cleared {
            print_dropped(entry, ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_of_first_line() {
        assert_eq!(line_of("foo\nbar\nbaz", SourceSpan::from((0, 1))), 1);
    }

    #[test]
    fn line_of_third_line() {
        // "foo\nbar\nbaz" — offset 8 lands on the 'a' of "baz" → line 3.
        assert_eq!(line_of("foo\nbar\nbaz", SourceSpan::from((8, 1))), 3);
    }

    #[test]
    fn line_of_offset_past_end_clamps_to_last_line() {
        // Defensive: an out-of-range offset shouldn't panic.
        assert_eq!(line_of("foo\nbar", SourceSpan::from((100, 1))), 2);
    }

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
