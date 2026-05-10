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

## Environment variables

Tools that take configuration from the environment (Docker, the
`OLLAMA_*` family, CUDA, ...) can be set up alongside flags. A KDL
node bearing the `(env)` type annotation declares an env var rather
than a CLI argument; profiles override defaults the same way they
override flags, and `(env)NAME #false` unsets a variable on the
child:

```kdl
llama-server "serve" {
    host "0.0.0.0"
    (env)OLLAMA_HOST "0.0.0.0"
    (env)CUDA_VISIBLE_DEVICES "0,1"

    qwen-coder {
        m "/models/qwen-coder.gguf"
        (env)CUDA_VISIBLE_DEVICES "0"
    }

    sandbox {
        m "/models/sandbox.gguf"
        (env)OLLAMA_HOST #false
    }
}
```

```text
$ jig --dry-run serve qwen-coder
env OLLAMA_HOST=0.0.0.0 CUDA_VISIBLE_DEVICES=0 llama-server --host 0.0.0.0 -m /models/qwen-coder.gguf

$ jig --dry-run serve sandbox
env -u OLLAMA_HOST CUDA_VISIBLE_DEVICES='0,1' llama-server --host 0.0.0.0 -m /models/sandbox.gguf
```

The `env(1)` prefix is the dry-run rendering only; actual execution
applies the same outcomes directly to the child via
`Command::env` / `Command::env_remove`. The child inherits `jig`'s
environment by default; declared sets and unsets layer on top.

## Listing

```text
$ jig --list
llama-server (alias: serve)
  default-args: --host 0.0.0.0 --port 8090 -c 32768 --flash-attn
  profiles:
    qwen-coder
    llama3
```

## Shell completion

`jig --completions <shell>` writes a completion script to stdout for
zsh, bash, or fish. The script tab-completes `jig`'s own flags as
well as the command names, aliases, and profile names defined in the
`jig.kdl` of whatever directory you're in (and forwards `--config
<PATH>` if you've passed one).

```sh
# zsh — drop into a directory on $fpath, then `compinit`:
jig --completions zsh > "${fpath[1]}/_jig"

# bash — source on shell startup:
jig --completions bash > ~/.local/share/bash-completion/completions/jig

# fish:
jig --completions fish > ~/.config/fish/completions/jig.fish
```

## Install

From [crates.io](https://crates.io/crates/jig-run):

```sh
cargo install jig-run
# installs the `jig` binary (the crate is published as `jig-run`
# because the bare `jig` name is taken by an unrelated utility).
```

Or build from source:

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
| `--completions <SHELL>` | Emit a completion script (zsh/bash/fish) with dynamic command/profile completion |
| `-h`, `--help`          | Print help                                                      |
| `-V`, `--version`       | Print version                                                   |

### Argument model

A KDL node with one value is a **flag**; a node with no value is a
**positional**. A `host "0.0.0.0"` line becomes `--host 0.0.0.0`;
a single-character key like `m "/path"` becomes `-m /path`; an
explicit-dash key like `-ngl 999` is passed verbatim. KDL booleans
toggle flag presence: `flash-attn #true` emits `--flash-attn`,
`flash-attn #false` suppresses it (even when it would otherwise
come from defaults).

A flag key may repeat within a scope — `gcc { I "/a"; I "/b" }`
resolves to `gcc -I /a -I /b`, and `-v -v -v` count flags work the
same way. When defaults *and* a profile both contribute a single
unmarked occurrence of the same key, the profile overrides (v1
behavior). To force *add* instead of *override* in that single+single
case, prefix the profile's key with `+`: `+I "/proj"`. See
[`SPEC.md`](./SPEC.md) for the full table.

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

See [`SPEC.md`](./SPEC.md) for the behavioral specification, which
covers:

- Config-file lookup precedence (`./jig.kdl` then `./.jig.kdl`, or
  `--config <PATH>`).
- The argument model — flags vs positionals, booleans, dash-quoted
  positionals.
- Prefix synthesis — when keys get `-` vs `--`.
- Defaults and profiles, and the merge semantics (first-occurrence
  positioning, repeated flag keys with single-mode vs repeat-mode
  resolution, `#false` suppression, and the `+` append marker).
- Constraints (uniqueness, no-leading-dash names).
- Diagnostic quality — what `jig` errors aim for.

[`IMPLEMENTATION.md`](./IMPLEMENTATION.md) covers the build, type
design, and dependency choices.

## Status

**v1, Unix only.** Tested on Linux and macOS. Windows support is
deferred — see [`FUTURE.md`](./FUTURE.md), which also tracks
environment-variable bindings, parent-directory config traversal,
profile inheritance, and other ideas surfaced during design.

## License

MIT — see [`LICENSE`](./LICENSE).
