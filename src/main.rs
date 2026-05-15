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
mod explain;
mod format;
mod list;
mod path;
mod resolve;
mod suggest;
mod theme;

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
        if let Ok(loaded) = config::load::load(cli.config.as_deref())
            && config::validate::validate(&loaded.config, &loaded.src).is_ok()
        {
            complete::print_commands(&loaded.config);
        }
        return Ok(0);
    }
    if let Some(name) = &cli.list_profiles {
        if let Ok(loaded) = config::load::load(cli.config.as_deref())
            && config::validate::validate(&loaded.config, &loaded.src).is_ok()
        {
            complete::print_profiles(&loaded.config, name);
        }
        return Ok(0);
    }

    let loaded = config::load::load(cli.config.as_deref())?;
    config::validate::validate(&loaded.config, &loaded.src)?;

    if cli.list {
        list::print(&loaded.config)?;
        return Ok(0);
    }

    let command_name = cli.command.as_deref().ok_or(Error::MissingCommand)?;

    if cli.explain {
        let (resolved, trace) = resolve::resolve_with_trace(
            &loaded.config,
            command_name,
            cli.profile.as_deref(),
            &loaded.config_dir,
        )?;
        let source_name = loaded.src.name();
        let source_bytes = loaded.src.inner();
        // Reconstruct the absolute config path so `--explain` can
        // render it relative to the user's cwd. `config_dir` is
        // absolute (per `load::absolutise_parent`) and `src.name()`
        // is the bare filename — joining them recovers the
        // canonical path used at load time.
        let source_path = loaded.config_dir.join(source_name);
        explain::print(
            &resolved,
            &trace,
            &cli.passthrough,
            source_name,
            &source_path,
            source_bytes,
        )?;
        return Ok(0);
    }

    let resolved = resolve::resolve(
        &loaded.config,
        command_name,
        cli.profile.as_deref(),
        &loaded.config_dir,
    )?;

    if cli.dry_run {
        let line = format::to_dry_run(
            &resolved.program,
            &resolved.args,
            &cli.passthrough,
            &resolved.env,
            resolved.cwd.as_deref(),
        )?;
        println!("{line}");
        Ok(0)
    } else {
        if !cli.quiet {
            // Pre-exec preview is informational (`SPEC.md` §3.4.1) —
            // a rendering failure here must not block the spawn.
            // Surface a short warning so the user knows the preview
            // was skipped, then continue. If the underlying problem
            // would also kill the spawn (e.g. a NUL byte in `cwd=`),
            // exec::run surfaces its own diagnostic shortly after.
            match format::to_dry_run(
                &resolved.program,
                &resolved.args,
                &cli.passthrough,
                &resolved.env,
                resolved.cwd.as_deref(),
            ) {
                Ok(line) => emit_preview(&line),
                Err(e) => emit_preview_unavailable(&e),
            }
        }
        let argv = format::to_argv(&resolved.args, &cli.passthrough);
        exec::run(
            &resolved.program,
            &argv,
            &resolved.env,
            resolved.cwd.as_deref(),
        )
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

/// Stderr fallback when preview rendering failed (e.g. non-UTF-8
/// argv or a NUL byte that `shlex` cannot quote). The preview is
/// informational and must never affect exit codes or the spawn, so
/// we replace the preview line with a one-line notice and let the
/// child run. `--dry-run` keeps the strict behavior — the line IS
/// the output there.
fn emit_preview_unavailable(err: &Error) {
    let _ = writeln!(std::io::stderr(), "jig: preview unavailable: {err}");
}
