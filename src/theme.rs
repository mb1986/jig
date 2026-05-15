//! Terminal styling shared by the human-readable output paths
//! (`--list`, `--explain`).
//!
//! A small fixed set of roles is styled — command names, profile
//! names, section labels, two annotation kinds, value text, and a
//! "dropped" role for entries that lost to a merge decision. Coloring
//! is a readability cue, not a syntax highlighter; flag keys and
//! values that come from the resolved-argv renderer stay on their
//! own default foreground.
//!
//! Each role is exposed as a method on [`Theme`] that returns a
//! styled string. [`Theme::Plain`] returns its input unchanged;
//! [`Theme::Ansi`] wraps it in the appropriate ANSI escape codes.
//! [`Theme::from_stdout`] picks the theme based on whether stdout
//! is a terminal and `NO_COLOR` is unset (the de-facto convention
//! from <https://no-color.org>).

use std::io::IsTerminal;

/// Terminal styling for human-readable output.
#[derive(Debug, Clone, Copy)]
pub enum Theme {
    /// No styling. Returns inputs unchanged.
    Plain,
    /// ANSI escape codes for terminal rendering.
    Ansi,
}

impl Theme {
    /// Pick a theme based on stdout: ANSI iff stdout is a terminal
    /// and `NO_COLOR` is unset. Otherwise plain.
    #[must_use]
    pub fn from_stdout() -> Self {
        if std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() {
            Self::Ansi
        } else {
            Self::Plain
        }
    }

    /// Bold blue — the command name on a header line.
    #[must_use]
    pub fn cmd_name(self, s: &str) -> String {
        match self {
            Self::Plain => s.to_string(),
            Self::Ansi => format!("\x1b[1;34m{s}\x1b[0m"),
        }
    }

    /// Italic magenta — profile names. Distinct hue and weight from
    /// the dim section labels so the two roles don't visually blend.
    #[must_use]
    pub fn profile_name(self, s: &str) -> String {
        match self {
            Self::Plain => s.to_string(),
            Self::Ansi => format!("\x1b[3;35m{s}\x1b[0m"),
        }
    }

    /// Dim (faint) — section labels, source positions, footnote
    /// markers. Lowered contrast rather than added weight so the
    /// values that follow stay the foreground content and the
    /// labels read as scaffolding.
    #[must_use]
    pub fn label(self, s: &str) -> String {
        match self {
            Self::Plain => s.to_string(),
            Self::Ansi => format!("\x1b[2m{s}\x1b[0m"),
        }
    }

    /// Green — `(alias: ...)` annotations on a command header.
    #[must_use]
    pub fn alias_annotation(self, s: &str) -> String {
        match self {
            Self::Plain => s.to_string(),
            Self::Ansi => format!("\x1b[32m{s}\x1b[0m"),
        }
    }

    /// Amber (256-color goldenrod) — `(extends ...)` annotations on
    /// inheriting profile headers, and the `(inherited)` annotation
    /// in `--explain` output for chain-ancestor contributors.
    #[must_use]
    pub fn extends_annotation(self, s: &str) -> String {
        match self {
            Self::Plain => s.to_string(),
            Self::Ansi => format!("\x1b[38;5;214m{s}\x1b[0m"),
        }
    }

    /// Bright white — the value rendered after a section label, and
    /// the resolved-argv echoes in `--explain`.
    #[must_use]
    pub fn value(self, s: &str) -> String {
        match self {
            Self::Plain => s.to_string(),
            Self::Ansi => format!("\x1b[97m{s}\x1b[0m"),
        }
    }

    /// Dim + strikethrough — values that lost to a merge decision
    /// (`--explain`'s "dropped" / "suppressed" rows). Pairs visually
    /// with `label` (also dim) so the row reads as background, then
    /// adds the crossed-out cue.
    #[must_use]
    pub fn dropped(self, s: &str) -> String {
        match self {
            Self::Plain => s.to_string(),
            Self::Ansi => format!("\x1b[2;9m{s}\x1b[0m"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_returns_input_unchanged() {
        let t = Theme::Plain;
        assert_eq!(t.cmd_name("llama-server"), "llama-server");
        assert_eq!(t.profile_name("qwen-coder"), "qwen-coder");
        assert_eq!(t.label("defaults:"), "defaults:");
        assert_eq!(t.alias_annotation("(alias: serve)"), "(alias: serve)");
        assert_eq!(t.extends_annotation("(extends qwen)"), "(extends qwen)");
        assert_eq!(t.value("--host 0.0.0.0"), "--host 0.0.0.0");
        assert_eq!(t.dropped("\"qwen.gguf\""), "\"qwen.gguf\"");
    }

    #[test]
    fn ansi_wraps_cmd_name_in_bold_blue() {
        assert_eq!(
            Theme::Ansi.cmd_name("llama-server"),
            "\x1b[1;34mllama-server\x1b[0m"
        );
    }

    #[test]
    fn ansi_wraps_profile_name_in_italic_magenta() {
        assert_eq!(
            Theme::Ansi.profile_name("qwen-coder"),
            "\x1b[3;35mqwen-coder\x1b[0m"
        );
    }

    #[test]
    fn ansi_wraps_label_in_dim() {
        assert_eq!(Theme::Ansi.label("args:"), "\x1b[2margs:\x1b[0m");
    }

    #[test]
    fn ansi_wraps_alias_annotation_in_green() {
        assert_eq!(
            Theme::Ansi.alias_annotation("(alias: serve)"),
            "\x1b[32m(alias: serve)\x1b[0m"
        );
    }

    #[test]
    fn ansi_wraps_extends_annotation_in_amber() {
        // Goldenrod 256-color, distinct from the alias-annotation
        // green so the two annotation kinds read apart on dense lines.
        assert_eq!(
            Theme::Ansi.extends_annotation("(extends qwen-coder)"),
            "\x1b[38;5;214m(extends qwen-coder)\x1b[0m"
        );
    }

    #[test]
    fn ansi_wraps_value_in_bright_white() {
        assert_eq!(
            Theme::Ansi.value("--host 0.0.0.0"),
            "\x1b[97m--host 0.0.0.0\x1b[0m"
        );
    }

    #[test]
    fn ansi_wraps_dropped_in_dim_strikethrough() {
        assert_eq!(
            Theme::Ansi.dropped("\"qwen.gguf\""),
            "\x1b[2;9m\"qwen.gguf\"\x1b[0m"
        );
    }
}
