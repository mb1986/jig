//! `jig` — run commands with arguments taken from a declarative
//! configuration file.
//!
//! See `SPEC.md` for the behavioral specification and `IMPLEMENTATION.md`
//! for the implementation guide. v1 is Unix-only.

#![warn(clippy::pedantic, clippy::nursery)]
#![deny(unsafe_code)]
#![warn(missing_docs)]

mod errors;

fn main() {
    // Scaffold only. Real wiring lands in the CLI step.
    std::process::exit(errors::ExitCode::JigFailure.as_i32());
}
