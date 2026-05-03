//! `--list` rendering per `SPEC.md` §3.4 / §7.1.
//!
//! Output format (non-normative, but we follow the §7.1 example):
//!
//! ```text
//! llama-server (alias: serve)
//!   default-args: --host 0.0.0.0 --port 8090 -c 32768 --flash-attn
//!   profiles:
//!     qwen-coder
//!     llama3
//!
//! rsync (alias: sync)
//!   default-args: --archive --verbose
//!   profiles:
//!     backup
//! ```
//!
//! Default-args are rendered through [`crate::format::format_args`]
//! so the `-` / `--` prefix synthesis (§2.5), `#true` / `#false`
//! handling (§2.4.1), and shell-quoting (§7.2) all match what a
//! `--dry-run` would produce.

use crate::config::{Argument, Command, CommandChild, Config};
use crate::errors::Result;
use crate::format;

/// Render `config` to stdout per `SPEC.md` §7.1.
///
/// # Errors
///
/// Returns [`crate::errors::Error::ArgumentContainsNul`] if any
/// default value contains a NUL byte (rare, but [`format_args`]
/// can't quote it).
///
/// [`format_args`]: crate::format::format_args
pub fn print(config: &Config) -> Result<()> {
    for (i, cmd) in config.commands.iter().enumerate() {
        if i > 0 {
            println!();
        }
        print_command(cmd)?;
    }
    Ok(())
}

fn print_command(cmd: &Command) -> Result<()> {
    match &cmd.alias {
        Some(alias) => println!("{} (alias: {alias})", cmd.name),
        None => println!("{}", cmd.name),
    }

    let defaults: Vec<Argument> = cmd
        .children
        .iter()
        .filter_map(|c| match c {
            CommandChild::Default(a) => Some(a.clone()),
            CommandChild::Profile { .. } => None,
        })
        .collect();
    if !defaults.is_empty() {
        let line = format::format_args(&defaults)?;
        println!("  default-args: {line}");
    }

    let profile_names: Vec<&str> = cmd
        .children
        .iter()
        .filter_map(|c| match c {
            CommandChild::Profile { name, .. } => Some(name.as_str()),
            CommandChild::Default(_) => None,
        })
        .collect();
    if !profile_names.is_empty() {
        println!("  profiles:");
        for name in profile_names {
            println!("    {name}");
        }
    }
    Ok(())
}
