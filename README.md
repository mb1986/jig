# jig

[![Crates.io](https://img.shields.io/crates/v/jig-run.svg)](https://crates.io/crates/jig-run)
[![CI](https://github.com/mb1986/jig/actions/workflows/ci.yml/badge.svg)](https://github.com/mb1986/jig/actions/workflows/ci.yml)
[![Crates.io MSRV](https://img.shields.io/crates/msrv/jig-run?style=flat)](https://crates.io/crates/jig-run)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A small CLI that runs commands using arguments declared in a config
file. You write a `jig.kdl` ([KDL][kdl]) describing the commands you
invoke often — defaults, named profiles, env vars, profile
inheritance — and `jig <command> [profile]` resolves the arguments
and executes. Conceptually adjacent to [`just`][just]: where `just`
runs arbitrary shell recipes, `jig` invokes specific programs with
declarative argument profiles.

[kdl]: https://kdl.dev
[just]: https://github.com/casey/just

## Example

`./jig.kdl`:

```kdl
// The string after the command name is an alias:
// `jig serve <profile>` and `jig api-server <profile>` are equivalent.
api-server "serve" {
    // ── Defaults — applied to every profile under this command ──
    host "0.0.0.0"                       // 2+ char key   →  --host
    port 8080                            //                →  --port
    log-level "info"
    v #true                              // 1-char key, no value emitted  →  -v

    // Explicit-dash keys pass through verbatim — for tools that don't
    // follow POSIX flag conventions (Go, ffmpeg, llama.cpp …).
    -shutdown-timeout "30s"              //                →  -shutdown-timeout 30s

    // Env vars on the spawned child. Profiles can override or unset.
    (env)RUST_LOG "info,api=debug"

    // Repeating a key emits every occurrence in source order.
    cors-origin "app.dev"
    cors-origin "admin.dev"

    // Single default — profiles can either replace it (single-mode
    // override) or append with the `+` marker.
    header "X-Service: api"

    // `#null` declares a flag at this slot but emits no value.
    // Profiles fill it in; the flag appears at this slot regardless
    // of where the supplying profile body sits in the file.
    tls-cert #null
    tls-key  #null

    // ── Profiles. `jig serve <name>` selects one. ──
    dev {
        port 3000                        // single-mode override
        log-level "debug"
        header "X-Service: api-dev"      // replaces the default header
    }

    // `extends=` inherits the parent profile, then layers on top.
    dev-trace extends="dev" {
        log-level "trace"
        pretty #true                     // add a new flag
    }

    prod {
        host "127.0.0.1"
        log-level "warn"
        v #false                         // suppress the default
        metrics #true
        +header "X-Region: eu"           // `+` appends — both headers emit
        (env)RUST_LOG #false             // unset the env var on the child
    }

    tls-staging {
        tls-cert "/etc/ssl/staging.pem"  // fills the #null placeholder
        tls-key  "/etc/ssl/staging.key"
    }
}

// Positionals are bare node names with no value — passed through
// verbatim in source order.
rsync "sync" {
    archive #true
    verbose #true

    home {
        "/home/me/"                      // positional (source)
        "backup:/snapshots/me/"          // positional (destination)
    }
}
```

From the same directory:

```console
$ jig --dry-run serve dev-trace
env RUST_LOG='info,api=debug' api-server --host 0.0.0.0 --port 3000 --log-level trace -v -shutdown-timeout 30s --cors-origin app.dev --cors-origin admin.dev --header 'X-Service: api-dev' --pretty

$ jig --dry-run serve prod
env -u RUST_LOG api-server --host 127.0.0.1 --port 8080 --log-level warn -shutdown-timeout 30s --cors-origin app.dev --cors-origin admin.dev --header 'X-Service: api' --metrics --header 'X-Region: eu'

$ jig --dry-run serve tls-staging
env RUST_LOG='info,api=debug' api-server --host 0.0.0.0 --port 8080 --log-level info -v -shutdown-timeout 30s --cors-origin app.dev --cors-origin admin.dev --header 'X-Service: api' --tls-cert /etc/ssl/staging.pem --tls-key /etc/ssl/staging.key

$ jig --dry-run sync home
rsync --archive --verbose /home/me/ backup:/snapshots/me/

$ jig serve dev
# spawns api-server with the resolved args, inheriting stdio
```

`--dry-run` output is shell-quoted so you can copy-paste it and get
the same effect.

For the full grammar — argument model, prefix synthesis, profile
inheritance, `#null` placeholders, merge semantics, env-var rules,
constraints, and diagnostic guarantees — see [`SPEC.md`](SPEC.md).

## Usage

```text
jig [FLAGS]... <command-or-alias> [profile] [PASSTHROUGH]...
```

| Flag                    | What it does                                                       |
|-------------------------|--------------------------------------------------------------------|
| `-n`, `--dry-run`       | Print the resolved (shell-quoted) command line and exit.           |
| `--config <PATH>`       | Use `<PATH>` instead of `./jig.kdl` / `./.jig.kdl`.                |
| `-l`, `--list`          | List configured commands, aliases, env vars, and profiles.        |
| `--completions <SHELL>` | Emit a shell completion script (`zsh`, `bash`, `fish`) to stdout.  |
| `-h`, `--help`          | Print help.                                                        |
| `-V`, `--version`       | Print version.                                                     |

Anything after the profile is appended verbatim to the resolved
command line, including a literal `--` and tokens that look like
flags. A single `--` written immediately after the command/alias is
consumed as a "no profile selected" marker; a `--` anywhere later
passes through unchanged.

`jig` follows the wrapper-tool exit-code convention used by `env(1)`,
`timeout(1)`, `nohup(1)`: `0` on success, `125` for `jig`'s own
errors (missing config, parse error, unknown command/profile, bad
CLI), `126` when the resolved command is found but not executable,
`127` when it is not found, and anything else propagated verbatim
from the child.

## Install

From [crates.io](https://crates.io/crates/jig-run):

```sh
cargo install jig-run
```

The crate is published as `jig-run`; the installed binary is `jig`.

The latest commit, straight from git:

```sh
cargo install --git https://github.com/mb1986/jig
```

Or build from source:

```sh
git clone https://github.com/mb1986/jig
cd jig
cargo build --release
# binary lands in target/release/jig
```

Edition 2024. No nightly features. Tested on Linux and macOS.

## Shell completion

`jig --completions <shell>` writes a completion script to stdout for
`zsh`, `bash`, or `fish`. The script tab-completes `jig`'s own flags
and the command names, aliases, and profile names defined in the
`jig.kdl` of whatever directory you're in (and forwards
`--config <PATH>` if you've passed one).

```sh
# zsh — drop into a directory on $fpath, then `compinit`:
jig --completions zsh > "${fpath[1]}/_jig"

# bash:
jig --completions bash > ~/.local/share/bash-completion/completions/jig

# fish:
jig --completions fish > ~/.config/fish/completions/jig.fish
```

## License

MIT — see [`LICENSE`](LICENSE).
