//! Did-you-mean suggestion helpers used by `resolve` (CLI lookup
//! errors) and `validate` (unknown `extends` parent). The threshold
//! is tuned so a one- or two-character typo on a short identifier
//! still surfaces a suggestion, while completely unrelated names
//! produce none.

/// Format the `help` field rendered with each "unknown X" diagnostic.
///
/// `label` is the plural noun for the kind being looked up
/// (`"commands"`, `"profiles"`); `available` is the list of valid
/// names; `did_you_mean` is an optional pre-filtered nearest match.
#[must_use]
pub fn build_help(label: &str, available: &[&str], did_you_mean: Option<&str>) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    if available.is_empty() {
        let _ = write!(s, "no {label} are defined in this config");
    } else {
        let _ = write!(s, "available {label}: {}", available.join(", "));
    }
    if let Some(suggestion) = did_you_mean {
        s.push('\n');
        let _ = write!(s, "did you mean {suggestion:?}?");
    }
    s
}

/// Return the nearest entry in `haystack` to `needle` by edit
/// distance, if any are within a small threshold. The threshold is
/// `max(2, len/3)` so longer names tolerate more drift; this matches
/// the heuristic used for command and profile lookups.
#[must_use]
pub fn nearest<'a>(needle: &str, haystack: &[&'a str]) -> Option<&'a str> {
    let threshold = 2.max(needle.chars().count() / 3);
    haystack
        .iter()
        .map(|s| (*s, levenshtein(needle, s)))
        .filter(|(_, d)| *d <= threshold)
        .min_by_key(|(_, d)| *d)
        .map(|(s, _)| s)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur: Vec<usize> = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_off_by_one_within_threshold() {
        assert_eq!(
            nearest("qwen-codr", &["qwen-coder", "llama3"]),
            Some("qwen-coder")
        );
    }

    #[test]
    fn nearest_completely_unrelated_returns_none() {
        assert_eq!(nearest("totally-unrelated", &["foo", "bar"]), None);
    }

    #[test]
    fn build_help_empty_available_says_none() {
        let s = build_help("profiles", &[], None);
        assert!(s.contains("no profiles are defined"));
    }

    #[test]
    fn build_help_with_suggestion_includes_did_you_mean() {
        let s = build_help("profiles", &["fast", "slow"], Some("fast"));
        assert!(s.contains("available profiles: fast, slow"));
        assert!(s.contains("did you mean \"fast\""));
    }
}
