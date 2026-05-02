//! Command-line argument parsing.
//!
//! Step 3 introduces the minimum CLI surface needed to exercise
//! load + parse + diagnostic rendering: `--config <PATH>` per
//! `SPEC.md` §2.1 and `--list` per §3.4. The struct grows with
//! later steps (`command`, `profile`, pass-through, `--dry-run`,
//! `--completions`, etc.).

use std::path::PathBuf;

use clap::Parser;

/// `jig` — run commands with arguments taken from a declarative
/// configuration file.
#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
    /// Use `<PATH>` instead of looking for `jig.kdl` / `.jig.kdl`
    /// in the current working directory. Per `SPEC.md` §2.1.
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// List all configured commands, aliases, and profiles.
    /// Per `SPEC.md` §3.4 / §7.1.
    #[arg(short = 'l', long)]
    pub list: bool,
}
