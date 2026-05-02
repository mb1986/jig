//! Wrapper-tool exit codes per `SPEC.md` §3.5.
//!
//! Step 2 introduces only the codes `jig` itself originates today —
//! currently just [`ExitCode::JigFailure`]. The remaining §3.5 codes
//! (`NotExecutable`, `NotFound`), the typed `Error` enum, the crate
//! `Result` alias, and `miette` rendering all land in later steps,
//! alongside their first real call sites.

/// Wrapper-tool exit codes per `SPEC.md` §3.5.
///
/// Successful runs and propagated child exit codes are plain `i32`
/// values returned out of the success path; this enum names only
/// the codes that `jig` itself originates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    /// `jig` itself failed: missing config, parse error, constraint
    /// violation, unknown command/profile/alias, bad CLI usage.
    JigFailure,
}

impl ExitCode {
    /// Numeric value, suitable for `std::process::exit`.
    #[must_use]
    pub const fn as_i32(self) -> i32 {
        match self {
            Self::JigFailure => 125,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jig_failure_is_125() {
        assert_eq!(ExitCode::JigFailure.as_i32(), 125);
    }
}
