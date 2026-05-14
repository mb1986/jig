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
mod complete;
mod completions;
mod config;
mod errors;
mod exec;
mod format;
mod list;
mod resolve;
mod suggest;

use std::io::{IsTerminal, Write};

use clap::CommandFactory;

use crate::cli::Cli;
use crate::errors::{Error, ExitCode, Result};

fn main() {
    let parsed = cli::parse_argv();
    let exit = match run(&parsed) {
        Ok(code) => code,
        Err(Error::MissingCommand) => {
            // Per Q2: print help to stderr and exit 125 as a
            // usage error, bypassing the standard miette render.
            let mut cmd = Cli::command();
            let _ = cmd.write_help(&mut std::io::stderr());
            let _ = writeln!(std::io::stderr());
            ExitCode::JigFailure.as_i32()
        }
        Err(e) => {
            let code = e.exit_code().as_i32();
            eprintln!("{:?}", miette::Report::new(e));
            code
        }
    };
    std::process::exit(exit);
}

fn run(cli: &Cli) -> Result<i32> {
    // `--completions` is handled before any config load, per Q1.
    if let Some(shell) = cli.completions {
        crate::cli::emit_completions(shell);
        return Ok(0);
    }

    // Candidate-emission flags (used by completion scripts) load
    // the config defensively: any error becomes empty stdout +
    // exit 0 so the shell never sees a half-broken tab-complete.
    if cli.list_commands {
        if let Ok((config, src)) = config::load::load(cli.config.as_deref())
            && config::validate::validate(&config, &src).is_ok()
        {
            complete::print_commands(&config);
        }
        return Ok(0);
    }
    if let Some(name) = &cli.list_profiles {
        if let Ok((config, src)) = config::load::load(cli.config.as_deref())
            && config::validate::validate(&config, &src).is_ok()
        {
            complete::print_profiles(&config, name);
        }
        return Ok(0);
    }

    let (config, src) = config::load::load(cli.config.as_deref())?;
    config::validate::validate(&config, &src)?;

    if cli.list {
        list::print(&config)?;
        return Ok(0);
    }

    let command_name = cli.command.as_deref().ok_or(Error::MissingCommand)?;
    let resolved = resolve::resolve(&config, command_name, cli.profile.as_deref())?;

    if cli.dry_run {
        let line = format::to_dry_run(
            &resolved.program,
            &resolved.args,
            &cli.passthrough,
            &resolved.env,
        )?;
        println!("{line}");
        Ok(0)
    } else {
        if !cli.quiet {
            let line = format::to_dry_run(
                &resolved.program,
                &resolved.args,
                &cli.passthrough,
                &resolved.env,
            )?;
            emit_preview(&line);
        }
        let argv = format::to_argv(&resolved.args, &cli.passthrough);
        exec::run(&resolved.program, &argv, &resolved.env)
    }
}

/// Write the pre-exec preview line to stderr per `SPEC.md` §3.4.1.
/// Bolded with ANSI `\x1b[1m…\x1b[0m` when stderr is a terminal;
/// plain text otherwise so redirected logs stay readable. A failed
/// write is silently ignored: the preview is purely informational
/// and must not affect exit codes or the spawn.
fn emit_preview(line: &str) {
    let mut stderr = std::io::stderr();
    let _ = if stderr.is_terminal() {
        writeln!(stderr, "\x1b[1m{line}\x1b[0m")
    } else {
        writeln!(stderr, "{line}")
    };
}
