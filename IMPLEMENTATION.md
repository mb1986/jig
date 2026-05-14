# jig — Implementation Guide

This document captures the implementation-level decisions made for `jig` v1. It is the companion to `SPEC.md` (which defines *what* `jig` does); this document defines *how* it is built.

## 1. Project Identity

| Layer | Name |
|---|---|
| Repository / directory | `jig` |
| Binary | `jig` |
| Crate (`Cargo.toml` `name`) | `jig-run` |
| GitHub URL | `github.com/<owner>/jig` |

Rationale: `jig` is the user-facing identity. `jig-run` is a crates.io publishing artifact (the bare `jig` name on crates.io is taken by an unrelated, low-traction utility). This mirrors the well-known precedent of `fd` (binary `fd`, repo `fd`, crate `fd-find`).

## 2. License

**MIT.**

A standard `LICENSE` file at the repo root and `license = "MIT"` in `Cargo.toml`. No source-file headers required for a project of this scope.

Apache-2.0 (or dual MIT/Apache-2.0) was considered but rejected: the patent-grant and GPLv2-compatibility concerns that motivate dual-licensing in larger projects do not apply here.

## 3. Language and Toolchain

- **Language:** Rust, edition 2024.
- **MSRV:** explicitly pinned in `Cargo.toml` (`rust-version = "1.88"` — the floor is edition 2024 plus stable let-chain expressions; bump only when a feature in use moves it).
- **No nightly features.** Stable Rust only.

## 4. Dependencies

Direct dependencies are kept minimal and justified.

| Crate | Purpose | Notes |
|---|---|---|
| `kdl` (v6.x) | Parse `jig.kdl` | Enable `v1-fallback` feature so users can write either v1 or v2 syntax. Produces `miette`-compatible diagnostics. |
| `clap` (v4, derive feature) | CLI argument parsing | Use `trailing_var_arg` and `allow_hyphen_values` for pass-through args. The `Shell` value-enum for `--completions` is hand-defined (zsh/bash/fish only), so no separate completion crate is needed. |
| `miette` | Diagnostic rendering | Source-span-aware error messages. Matches `kdl`'s diagnostic ecosystem. Enable the `fancy` feature for terminal rendering. |
| `thiserror` | Typed error definitions | Used together with `miette` derives. |
| `shlex` | Shell-quoting for `--dry-run` output | `shlex::try_quote` produces POSIX-compatible quoting. |

Dev-dependencies (in `[dev-dependencies]`):

| Crate | Purpose |
|---|---|
| `assert_cmd` | End-to-end binary invocation in integration tests. |
| `predicates` | Assertions on stdout/stderr in integration tests (paired with `assert_cmd`). |
| `insta` | Snapshot tests for `--dry-run` output and rendered error messages. |

Explicitly **not** depended on:

- `anyhow` — conflicts ergonomically with `miette`/`thiserror`. Our typed `Error` enum is sufficient.
- `serde` / `serde_*` — no serialization needed.
- `tokio` / `async-std` — `jig` is synchronous; `std::process::Command` is the right tool.
- `directories` / `etcetera` — config lookup is CWD-only with two filenames; no path-discovery library needed.
- `tracing` / `log` — error messages go directly to stderr; no logging framework required.

## 5. Project Structure

Single binary crate with clear internal modules:

```
src/
  main.rs          // entry point, wires CLI → resolve → exec
  cli.rs           // clap derive definitions
  config/
    mod.rs         // public types: Config, Command, CommandChild, Profile, Argument
    parse.rs       // KDL document → Config
    validate.rs    // constraint checks (alias uniqueness, name collisions, etc.)
  resolve.rs       // command/alias lookup + source-order walk with profile selection → resolved argument list
  format.rs        // resolved argument list → Vec<OsString> for execv
  exec.rs          // run resolved command, propagate exit status / signals
  list.rs          // --list rendering
  complete.rs      // candidate emitters for --list-commands / --list-profiles
  completions/
    mod.rs         // Shell value-enum + dispatch to embedded scripts
    jig.zsh        // hand-rolled zsh completion script
    jig.bash       // hand-rolled bash completion script
    jig.fish       // hand-rolled fish completion script
  errors.rs        // typed error enum with miette + thiserror derives
tests/
  integration.rs   // end-to-end tests via assert_cmd
```

A library/binary split is not introduced for v1. It is a non-breaking refactor if needed later.

## 6. Code Quality Standards

### 6.1 Lints

In `src/main.rs` (or `lib.rs` if added later):

```rust
#![warn(clippy::pedantic, clippy::nursery)]
#![deny(unsafe_code)]
#![warn(missing_docs)]
```

Targeted `#[allow]` for individual `clippy::pedantic` lints is acceptable but must be justified with a comment.

`unsafe` is forbidden. There is no scenario in `jig` where `unsafe` is warranted.

### 6.2 Error handling

- **No `unwrap()` or `expect()` in non-test code,** with one narrow exception: `expect("invariant: <reason>")` is permitted when an outcome is guaranteed by upstream invariants and the message documents the reason. Bare `unwrap()` is never permitted in non-test code.
- All fallible operations propagate via `?` into the crate-level `Error` type.
- All errors targeting users render through `miette` with source spans, labels, and help text.

### 6.3 Cloning and allocation

- `clone()` calls in cold paths for clarity are acceptable.
- `clone()` calls used to evade borrow-checker friction are not. Restructure the code instead.
- Hot paths (resolution, argument formatting) prefer borrows and iterators over owned collections where it does not hurt readability.

### 6.4 Dead code

No `#[allow(dead_code)]` "for later." If code is not used by v1, it is not in v1. Speculative abstractions, framework-style hooks, and "we might want this someday" code are explicitly out of scope.

### 6.5 Documentation

- Every public item (in `config`, `resolve`, etc.) has a doc comment, even though all modules are internal in v1. The `missing_docs` lint enforces this.
- Doc comments explain *why*, not just *what*.
- The crate-level doc comment in `main.rs` summarizes what `jig` does and links to `SPEC.md`.

## 7. Type Design

### 7.1 String handling

KDL guarantees UTF-8, so config-sourced values are `String`. Pass-through arguments come from `std::env::args_os()` and are `OsString`. The resolved argument list passed to `Command::args` therefore mixes both — its element type is `OsString`. Conversion happens at the format-layer boundary, not earlier.

### 7.2 Argument representation

`Argument` is an enum that models the spec exactly:

```rust
enum Argument {
    Flag { key: FlagKey, value: FlagValue },
    Positional(String),
}

enum FlagKey {
    /// Key without an explicit dash prefix; gets `-` (1 char) or `--` (2+).
    Inferred(String),
    /// Key written with explicit dash prefix in the config; passed verbatim.
    Verbatim(String),
}

enum FlagValue {
    /// Boolean keyword `#true` or `#false` from KDL. Drives include/suppress.
    Bool(bool),
    /// The KDL `#null` keyword. Position-only placeholder (`SPEC.md` §2.4.3):
    /// declares the flag at this source position but contributes no value,
    /// suppresses nothing, never emits. Its idx feeds the first-occurrence
    /// pool in the per-key merge (`SPEC.md` §2.8 step 2.4 / §2.8.5 step 4).
    Null,
    /// Any non-boolean, non-null value: strings, integers, floats. Stored as
    /// the textual representation that should appear on the command line, so
    /// no precision/rounding concerns and no need to round-trip numeric types.
    Literal(String),
}
```

Storing non-boolean values as their textual representation (rather than parsing numbers into `i64`/`f64`) avoids precision loss for floats like `0.5` and integer-vs-float ambiguity for values like `8090`. The KDL parser exposes the original source text for each value, which is what we keep.

Implementation hint: the `kdl` crate's `KdlValue` carries both a parsed value and an optional original-source representation. When constructing a `FlagValue::Literal`, prefer the raw source text where available; for values that have no original repr (e.g. constructed programmatically — not our case), fall back to a canonical formatting of the parsed value. For our purposes (we only ever read parsed config), the original-source path is always taken.

This shape lets pattern-matching during merge and format steps be exhaustive and trivially correct.

### 7.3 Config representation

`Config` is the parsed-and-validated structure. Raw `kdl::KdlDocument` does not leak past the parse boundary. Every constraint listed in `SPEC.md` §2.9 is checked during validation and converted into a typed error variant. Downstream code (resolve, format, exec) operates only on the validated form and cannot encounter "shape" errors.

#### 7.3.1 Command body as a source-ordered list

A `Command`'s body is represented as a single source-ordered `Vec` of children, not as separate `defaults` and `profiles` collections:

```rust
struct Command {
    name: String,
    alias: Option<String>,
    children: Vec<CommandChild>,
}

enum CommandChild {
    Default(Argument),
    Profile {
        name: String,
        /// Optional parent profile (`SPEC.md` §2.8.5). When set,
        /// the parent's body activates alongside this profile's at
        /// resolution time, with the parent emitting at its own
        /// source slot.
        extends: Option<(String, SourceSpan)>,
        args: Vec<Argument>,
    },
}
```

This directly mirrors the spec's wording ("walk the command's children in source order") and makes the resolve algorithm a one-line iteration. Profile name uniqueness (per `SPEC.md` §2.9) is enforced during validation by a separate pass over the `children` vec, not by the type itself.

This representation also keeps the door open for `FUTURE.md`'s "repeating the same profile within a command" feature: lifting the uniqueness constraint becomes a matter of deleting the validation pass, with no changes required to the walk or the types. The naive alternative — `defaults: Vec<Argument>` plus `profiles: HashMap<String, Profile>` — would require restructuring the type to allow duplicates, so we avoid it.

Profile lookup by name is implemented as a linear scan of `children`. This is O(n), but n is tiny (typically < 20 profiles per command), and the tradeoff favors structural simplicity over micro-optimization.

Inheritance is implemented as an N-tier extension of the merge algorithm rather than a separate code path. `resolve()` walks `extends` pointers to build the chain `[root, …, leaf]`, tags each profile-in-chain's body candidates with a tier index (defaults = 0, root ancestor = 1, …, leaf = N), and runs the §2.8.5 per-key cascade. The two-tier rules degenerate cleanly when no inheritance is used; every pre-inheritance test passes byte-identical. Cycle detection and unknown-parent diagnostics live in `config::validate`, so `resolve` can assume an acyclic chain and panic via `assert!` if that invariant is broken (defence-in-depth for unvalidated callers).

## 8. Error Reporting

Implements `SPEC.md` §7.4 with the `miette` ecosystem:

- The crate-level `Error` type derives `thiserror::Error` and `miette::Diagnostic`.
- Each variant attaches `#[source_code]` (the loaded `jig.kdl` text), `#[label]`s pointing at relevant spans, and `#[help]` strings with suggested fixes.
- KDL parse errors are surfaced via `#[diagnostic(transparent)]` — the `kdl` crate's spans and rendering are preserved, prefixed only with the config file path.
- Where errors involve two source locations (e.g. duplicate aliases), both spans are labeled.
- "Did you mean?" suggestions for unknown command/profile/alias use a nearest-name heuristic (e.g. Levenshtein distance) over the set of valid names.

Errors are written to stderr. Exit codes follow `SPEC.md` §3.5 (125 for any `jig`-internal failure, 126/127 for exec problems, propagated otherwise).

## 9. CLI Implementation Notes

### 9.1 Pass-through parsing

The CLI struct uses `trailing_var_arg = true` and `allow_hyphen_values = true` on the pass-through field, so positional args after the profile name (including ones that look like flags) are collected verbatim:

```rust
#[derive(clap::Parser)]
struct Cli {
    #[arg(short = 'n', long)]
    dry_run: bool,

    #[arg(long)]
    config: Option<PathBuf>,

    #[arg(short = 'l', long)]
    list: bool,

    #[arg(long = "completions", value_enum, hide = true)]
    completions: Option<clap_complete::Shell>,

    /// Command name or alias.
    command: Option<String>,

    /// Profile name (optional).
    profile: Option<String>,

    /// Arguments appended verbatim to the resolved command line.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    passthrough: Vec<OsString>,
}
```

A literal `--` token in `passthrough` is preserved (clap consumes the `--` separator only when it appears immediately after the last named argument; subsequent `--` tokens are kept).

### 9.2 Completion generation

`--completions <SHELL>` emits a hand-rolled completion script for `zsh`, `bash`, or `fish`. Each script is stored as a verbatim file under `src/completions/` (`jig.zsh`, `jig.bash`, `jig.fish`) and embedded into the binary via `include_str!`. The flag is hidden from `--help` because it is rarely used directly by humans.

Users typically run `jig --completions zsh > /path/to/completion/dir/_jig` once during shell setup. Distribution-level packaging may pre-install the scripts.

Each script knows `jig`'s own flags statically and dispatches to two hidden completion-only flags for dynamic candidates:

- `jig --list-commands` — one candidate per line, listing every alias plus every command name that appears exactly once. Duplicated bare names are excluded because they are not valid lookup keys (per `SPEC.md` §2.9 / §4).
- `jig --list-profiles <COMMAND>` — one profile name per line for the matched command (resolved by name or alias). An unknown name or a duplicated bare name produces empty output.

Both flags exit `0` with empty stdout and empty stderr on any failure (missing config, parse error, validation error) so completion never breaks mid-tab. The candidate emitters live in `src/complete.rs` and are wired into `main::run` ahead of the normal command-resolution flow.

The shell scripts forward an explicit `--config <PATH>` from the user's command line to the candidate-emission calls, so completion always reflects the chosen config. Other shells (`elvish`, `powershell`, …) are intentionally unsupported in v1; adding one is a matter of dropping a fourth script under `src/completions/` and a variant on the `Shell` enum.

### 9.3 Execution

The resolved command is launched via `std::process::Command`. `jig` waits for the child and exits with the child's exit status (mapping signal-killed children to `128 + signum` per shell convention).

Standard streams (stdin, stdout, stderr) are inherited. Signals delivered to `jig` (SIGINT, SIGTERM) reach the child naturally because the child is in the same process group; no explicit signal forwarding is required for the v1 use cases.

## 10. Build Configuration

### 10.1 `Cargo.toml`

```toml
[package]
name = "jig-run"
version = "0.1.0"
edition = "2024"
rust-version = "1.88"
license = "MIT"
description = "Run commands with arguments taken from a declarative configuration file."
repository = "https://github.com/<owner>/jig"
readme = "README.md"

[[bin]]
name = "jig"
path = "src/main.rs"

[profile.release]
lto = "thin"
codegen-units = 1
strip = true
```

`opt-level` is left at the release default (`3`). `jig` is invoked at human-interaction speed and does not need further tuning.

### 10.2 `rustfmt.toml`

Empty file (use defaults), or absent. We deviate from defaults only with explicit reason.

### 10.3 `clippy.toml`

Empty file (or absent). Lint configuration lives in source via `#![warn]` / `#![deny]` attributes.

## 11. Testing Strategy

- **Unit tests:** colocated `#[cfg(test)] mod tests` in each module (`config::parse`, `config::validate`, `resolve`, `format`).
- **Snapshot tests:** `insta` for `--dry-run` output and error-message rendering. Snapshots make diagnostic regressions visible in PRs without per-character assertions.
- **Integration tests:** in `tests/`, using `assert_cmd` and `predicates`. One test per major spec behavior:
  - Alias resolution (command name and alias both work)
  - Profile defaults (no profile)
  - Profile override (string/number/bool)
  - First-occurrence positioning (override at default's slot when default is first; at profile's slot when profile is first)
  - Profiles as positional slots (defaults written after a profile appear after that profile's args in the resolved command)
  - `#false` suppression (including non-boolean defaults)
  - Pass-through argument placement
  - Source-order argument emission (flags and positionals interleaved per config)
  - Exit codes (125 for missing config, 127 for missing binary, propagated otherwise)
  - Boolean vs string distinction (`#true` vs `"true"`)
  - Profile inheritance via `extends="<parent>"` (`SPEC.md` §2.8.5): cascade override, forward declarations, sibling non-leak; cycle and unknown-parent diagnostics rendered through the binary
  - Completion script generation (`--completions zsh|bash|fish` produces non-empty script and exits 0)
  - Dynamic completion candidate emission (`--list-commands` / `--list-profiles`): correct outputs for unique names, aliases, duplicated bare names; silent exit-0 for missing/malformed configs; `--config <PATH>` forwarding
- **Fuzz testing:** out of scope for v1. The KDL parser itself is fuzzed upstream.

## 12. Out of Scope for v1

The following are deliberately deferred and tracked here so they are not silently re-introduced:

- Library crate / programmatic API.
- Parent-directory configuration traversal.
- Global / per-user configuration.
- Environment variable interpolation in values.
- Templating, includes, or computed values.
- Multi-parent / diamond inheritance between profiles (single-parent `extends=` is supported per `SPEC.md` §2.8.5).
- Multiple aliases per command.
- JSON or other machine-readable `--list` format.
- Argv-style `--dry-run` output.
- A `--show` / `--explain` flag distinct from `--dry-run`.
- Logging or tracing infrastructure.

Adding any of these later is a non-breaking change to the v1 design.
