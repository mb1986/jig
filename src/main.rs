//! `jig` — run commands with arguments taken from a declarative
//! configuration file.
//!
//! See `SPEC.md` for the behavioral specification and `IMPLEMENTATION.md`
//! for the implementation guide. v1 is Unix-only.

#![warn(clippy::pedantic, clippy::nursery)]
#![deny(unsafe_code)]
#![warn(missing_docs)]
// Diagnostic-rich error variants carry `NamedSource<String>` inline
// so spans render against the right file (per `SPEC.md` §7.4).
// Boxing every `Err` propagation to satisfy `result_large_err`
// would only add allocations on the cold path.
#![allow(clippy::result_large_err)]

mod cli;
mod config;
mod errors;
mod list;

use clap::Parser;

use crate::cli::Cli;
use crate::errors::Result;

fn main() {
    let parsed = Cli::parse();
    let exit = match run(&parsed) {
        Ok(()) => 0,
        Err(e) => {
            let code = e.exit_code().as_i32();
            // miette's default Debug impl on Report uses the
            // configured handler, which falls back to a graphical
            // terminal-aware renderer when the `fancy` feature is
            // on.
            eprintln!("{:?}", miette::Report::new(e));
            code
        }
    };
    std::process::exit(exit);
}

fn run(args: &Cli) -> Result<()> {
    let (config, src) = config::load::load(args.config.as_deref())?;
    config::validate::validate(&config, &src)?;
    if args.list {
        list::print(&config);
    }
    // No command/profile execution yet — that lands in Step 5+.
    Ok(())
}
