# jig

Run commands with arguments taken from a declarative configuration
file.

`jig` is a profile/preset manager for command-line tools. You write
a small `jig.kdl` describing the commands you run often, the default
arguments they take, and named profiles that override or extend
those defaults. Then `jig <command> [profile]` assembles the
argument list and executes.

It is conceptually adjacent to [`just`][just] but distinct: where
`just` is a recipe runner that can execute arbitrary shell, `jig`
only assembles argument lists. One thing, well.

[just]: https://github.com/casey/just

## Example

`./jig.kdl`:

```kdl
llama-server "serve" {
    host "0.0.0.0"
    port 8090
    c 32768
    flash-attn #true

    qwen-coder {
        m "/models/qwen-coder.gguf"
        -ngl 999
        -ts "0.5,0.5"
    }

    llama3 {
        m "/models/llama3.gguf"
        port 8091
    }
}
```

From that directory:

```text
$ jig --dry-run serve qwen-coder
llama-server --host 0.0.0.0 --port 8090 -c 32768 --flash-attn -m /models/qwen-coder.gguf -ngl 999 -ts '0.5,0.5'

$ jig serve qwen-coder
# launches llama-server with those args, inheriting stdio
```

`jig serve` (no profile) launches with just the defaults; `jig serve
llama3` overrides `--port` and `-m` per the `llama3` profile.

`--dry-run` output is shell-quoted so you can copy-paste it into a
terminal and get the same effect.

## Listing

```text
$ jig --list
llama-server (alias: serve)
  default-args: --host 0.0.0.0 --port 8090 -c 32768 --flash-attn
  profiles:
    qwen-coder
    llama3
```

## Install

For now, build from source:

```sh
git clone https://github.com/mb1986/jig
cd jig
cargo build --release
# binary lands in target/release/jig
```

Rust 1.85+ (edition 2024). No nightly features.

## Usage

```text
jig [JIG_FLAGS]... <command-or-alias> [profile] [PASSTHROUGH]...
```

`<command-or-alias>` matches a command name or alias from `jig.kdl`.
`[profile]` selects a profile within that command. Anything after is
appended verbatim to the resolved command line, including a literal
`--` and tokens that look like flags.

| Flag                    | What it does                                                    |
|-------------------------|-----------------------------------------------------------------|
| `-n`, `--dry-run`       | Print the resolved (shell-quoted) command line and exit         |
| `--config <PATH>`       | Use `<PATH>` instead of `./jig.kdl` / `./.jig.kdl`              |
| `-l`, `--list`          | List configured commands, aliases, and profiles                 |
| `--completions <SHELL>` | Emit a static completion script (bash/zsh/fish/elvish/powershell) |
| `-h`, `--help`          | Print help                                                      |
| `-V`, `--version`       | Print version                                                   |

### Argument model

A KDL node with one value is a **flag**; a node with no value is a
**positional**. A `host "0.0.0.0"` line becomes `--host 0.0.0.0`;
a single-character key like `m "/path"` becomes `-m /path`; an
explicit-dash key like `-ngl 999` is passed verbatim. KDL booleans
toggle flag presence: `flash-attn #true` emits `--flash-attn`,
`flash-attn #false` suppresses it (even when it would otherwise
come from defaults). See `SPEC.md` §2.4 / §2.5 for the table.

### Exit codes

`jig` follows the wrapper-tool exit-code convention used by
`env(1)`, `timeout(1)`, `nohup(1)`:

| Code | Meaning                                                              |
|------|----------------------------------------------------------------------|
| 0    | Successful resolution and execution (or any `--dry-run` / `--list` / `--completions` / `--help` / `--version`) |
| 125  | `jig` itself failed (missing config, parse / constraint error, unknown command/profile/alias, bad CLI usage) |
| 126  | The resolved command was found but is not executable                 |
| 127  | The resolved command was not found                                   |
| else | Propagated verbatim from the executed command                        |

## Configuration

See [`SPEC.md`](./SPEC.md) for the behavioral specification:

- Lookup precedence (§2.1) — `./jig.kdl` then `./.jig.kdl`, or
  `--config <PATH>`.
- Argument model (§2.4) — flags vs positionals, booleans, dash-quoted
  positionals.
- Prefix synthesis (§2.5) — when keys get `-` vs `--`.
- Defaults and profiles (§2.7), merge semantics (§2.8) — first-occurrence
  positioning, `#false` suppression.
- Constraints (§2.9) — uniqueness, no-leading-dash names.
- Diagnostic quality (§7.4) — what `jig` errors aim for.

[`IMPLEMENTATION.md`](./IMPLEMENTATION.md) covers the build, type
design, and dependency choices.

## Status

**v1, Unix only.** Tested on Linux and macOS. Windows support is
deferred — see [`FUTURE.md`](./FUTURE.md), which also tracks
dynamic shell completion, environment-variable bindings, parent-
directory config traversal, profile inheritance, repeating flags,
and other ideas surfaced during design.

## License

MIT — see [`LICENSE`](./LICENSE).
