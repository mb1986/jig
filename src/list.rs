//! `--list` rendering.
//!
//! Step 3 ships a deliberately raw, debug-style listing that
//! exhaustively reads every field of [`Config`]. This both keeps
//! `cargo build` free of dead-code warnings before the resolver
//! and formatter exist, and gives the user immediate feedback
//! that their config parsed correctly. Step 5 (or 7) replaces
//! this with the polished §7.1 format once `format.rs` is
//! available to render args properly.

use crate::config::{Argument, Command, CommandChild, Config, FlagKey, FlagValue};

/// Print a raw listing of `config` to stdout.
pub fn print(config: &Config) {
    for cmd in &config.commands {
        print_command(cmd);
    }
}

fn print_command(cmd: &Command) {
    match &cmd.alias {
        Some(alias) => println!("{} (alias: {alias})", cmd.name),
        None => println!("{}", cmd.name),
    }
    for child in &cmd.children {
        match child {
            CommandChild::Default(arg) => println!("  default: {}", describe_arg(arg)),
            CommandChild::Profile { name, args, .. } => {
                println!("  profile {name}");
                for arg in args {
                    println!("    {}", describe_arg(arg));
                }
            }
        }
    }
}

fn describe_arg(arg: &Argument) -> String {
    match arg {
        Argument::Flag { key, value, .. } => {
            let key_str = match key {
                FlagKey::Inferred(s) => format!("inferred:{s}"),
                FlagKey::Verbatim(s) => format!("verbatim:{s}"),
            };
            let value_str = match value {
                FlagValue::Bool(true) => "#true".to_string(),
                FlagValue::Bool(false) => "#false".to_string(),
                FlagValue::Literal(s) => format!("{s:?}"),
            };
            format!("flag {key_str} = {value_str}")
        }
        Argument::Positional(s) => format!("positional {s:?}"),
    }
}
