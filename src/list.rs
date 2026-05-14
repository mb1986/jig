//! `--list` rendering per `SPEC.md` §3.4 / §7.1.
//!
//! Output format (non-normative, but we follow the §7.1 example):
//!
//! ```text
//! llama-server (alias: serve)
//!   env: -u OLD_VAR OLLAMA_HOST=0.0.0.0
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
//! `--dry-run` would produce. The `env:` line is emitted only when
//! the command declares default env vars (§2.10) and uses the same
//! `env(1)`-style form as `--dry-run` (`-u NAME` for unsets, then
//! `NAME=value` for sets).

use crate::config::{Argument, Command, CommandChild, Config, EnvValue};
use crate::errors::Result;
use crate::format;
use crate::resolve::EnvOp;

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

    if let Some((path, _)) = &cmd.cwd {
        // Show the cwd as written in the config (relative paths
        // stay relative, absolute stay absolute) per `SPEC.md` §7.1.
        println!("  cwd: {path}");
    }

    if !cmd.env.is_empty() {
        let env_ops: Vec<EnvOp> = cmd
            .env
            .iter()
            .map(|e| match &e.value {
                EnvValue::Set(v) => EnvOp::Set {
                    name: e.name.clone(),
                    value: v.clone(),
                },
                EnvValue::Unset => EnvOp::Unset {
                    name: e.name.clone(),
                },
            })
            .collect();
        let line = format::format_env(&env_ops)?;
        println!("  env: {line}");
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

    let profile_entries: Vec<(&str, Option<&str>)> = cmd
        .children
        .iter()
        .filter_map(|c| match c {
            CommandChild::Profile { name, extends, .. } => {
                Some((name.as_str(), extends.as_ref().map(|(p, _)| p.as_str())))
            }
            CommandChild::Default(_) => None,
        })
        .collect();
    if !profile_entries.is_empty() {
        println!("  profiles:");
        for (name, parent) in profile_entries {
            match parent {
                Some(p) => println!("    {name} (extends {p})"),
                None => println!("    {name}"),
            }
        }
    }
    Ok(())
}
