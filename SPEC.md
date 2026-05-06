# jig — Specification (v1, MVP)

## 1. Overview

`jig` is a command-line tool that runs other commands with arguments taken from a declarative configuration file. Configurations are organized as **named profiles** under **named commands**, with support for command aliases, default arguments, and per-profile overrides.

`jig` is conceptually adjacent to `just` but distinct: where `just` is a command runner that executes user-defined recipes (arbitrary shell), `jig` is a **profile/preset manager** that assembles a target command's argument list from structured data.

### 1.1 Goals

- Replace long, repetitive command invocations with short profile names.
- Allow shared default arguments across profiles of the same command.
- Use a config format that is more readable than the equivalent shell command.
- Stay focused: do one thing well — assemble argument lists.

### 1.2 Non-goals (for v1)

- Shell logic, piping, conditionals, or recipe dependencies — use `just` or shell scripts.
- Global configuration file or per-user config.
- Parent-directory configuration traversal.
- Environment variable expansion or templating.

## 2. Configuration File

### 2.1 Location

`jig` looks for a configuration file in the current working directory only:

1. `./jig.kdl` (preferred, visible)
2. `./.jig.kdl` (hidden fallback)

If both exist, `jig.kdl` is used and `.jig.kdl` is silently ignored.

If neither exists, `jig` exits with an error.

When the `--config <PATH>` flag is provided, the explicit path is used directly and the CWD lookup is skipped entirely. The path may be absolute or relative (relative paths are resolved against the CWD as usual).

### 2.2 Format

The configuration file is a [KDL](https://kdl.dev) document.

Top-level structure:

```kdl
<command-name> [<alias>] {
    [default-arguments]
    [profile-blocks]
}
```

- Each top-level node is a **command entry**.
- The node name is the **command** to be executed (it must be on `$PATH` or a path).
- An optional string argument on the node is the **alias**.
- Children of the command node are either **default arguments** or **profiles**, distinguished structurally.

### 2.3 Argument vs. profile distinction

A child node of a command or profile is:

- A **profile** if it has a child block (`{ ... }`).
- An **argument** otherwise (a flag if it has a value, a positional if it has none — see §2.4).

This rule is unambiguous because KDL distinguishes nodes-with-children from nodes-without.

A profile is itself a node with children; the children of a profile follow the same rules (flag if it has a value, positional if not). Profiles do not nest within profiles in v1.

### 2.4 Argument model

There are two kinds of arguments: **flags** and **positionals**. They are distinguished structurally by whether the KDL node has a value:

- **Node with a value** → flag. The node name becomes the flag key (with prefix rules per §2.5), and the value becomes the flag's value.
- **Node without a value** → positional. The node name is taken literally as the positional value.

This rule is unambiguous: every node either has at least one value or it has none.

| Config form               | Resolved CLI fragment        | Notes |
|---------------------------|------------------------------|-------|
| `host "0.0.0.0"`          | `--host 0.0.0.0`             | 2+ char key → `--` prefix |
| `m "/path/to/model"`      | `-m /path/to/model`          | 1 char key → `-` prefix   |
| `port 8090`               | `--port 8090`                | Numeric values OK         |
| `ts "0.5,0.5"`            | `--ts 0.5,0.5`               | Strings preserved as-is   |
| `-ngl 999`                | `-ngl 999`                   | Explicit dash → as-is     |
| `--explicit-flag "v"`     | `--explicit-flag v`          | Explicit dashes → as-is   |
| `flash-attn #true`        | `--flash-attn`               | KDL boolean → include flag |
| `flash-attn #false`       | (suppressed)                 | KDL boolean → omit flag |
| `test "true"`             | `--test true`                | String `"true"` is a literal value |
| `test "false"`            | `--test false`               | String `"false"` is a literal value |
| `"/some/path"`            | `/some/path`                 | No value → positional |
| `"output.mp4"`            | `output.mp4`                 | No value → positional |
| `"--"`                    | `--`                         | Literal `--` as positional |

> **Note on KDL syntax:** This spec uses KDL v2 syntax. In v2, boolean keywords are written as `#true` and `#false` (with the `#` sigil); in v1 they were bare `true` and `false`. The semantic distinction `#true` (boolean) vs `"true"` (string) is the same in both versions. We adopt v2 throughout; v1 syntax may be supported as a fallback if the chosen KDL parser allows it.

#### 2.4.1 Booleans vs string-valued flags

KDL distinguishes the boolean keyword `#true` from the string `"true"`. `jig` exploits this:

- A flag whose value is the **boolean** `#true` is included in the resolved command without an accompanying value (e.g. `--flash-attn`).
- A flag whose value is the **boolean** `#false` is **suppressed** from the resolved command, even if it would have been included via defaults.
- A flag whose value is the **string** `"true"` or `"false"` is a normal string-valued flag whose value happens to be the literal text `true` or `false` (e.g. `--test true`).

This lets users represent commands like `mytool --enabled true` (where `true` is a literal argument the tool parses) by writing `enabled "true"`, distinct from `mytool --enabled` (a flag with no value) written as `enabled #true`.

#### 2.4.2 Positional values that look like flags

Because positionals are bare node names with no value, a positional whose literal text starts with a dash is written as a quoted node name:

```kdl
profile {
    "--"
    "-stdin"
    "-1.5"
}
```

These are passed through verbatim as positional arguments. There is no ambiguity with flags because flags always have a value.

### 2.5 Flag prefix rules

The mapping from key name to CLI flag is:

1. If the key starts with `-` or `--`, it is passed verbatim.
2. Otherwise, if the key is exactly **1 character**, prefix with `-` (single dash).
3. Otherwise (2+ characters), prefix with `--` (double dash).

Rule 1 is the escape hatch for non-POSIX flag conventions like llama.cpp's `-ngl`, `-ts`, `-c:v`, etc.

### 2.6 Positional arguments

Positional arguments are KDL nodes with **no value**. The node name itself is the positional value:

```kdl
ffmpeg "transcode" {
    profile {
        i "input.mp4"
        -c:v "libx264"
        "output.mp4"
    }
}
```

Resolves to: `ffmpeg -i input.mp4 -c:v libx264 output.mp4`

Positionals whose literal value starts with a dash, contains whitespace, or is otherwise not a valid bare KDL identifier must be quoted:

```kdl
profile {
    "--"
    "-stdin"
    "/path/with spaces/file"
}
```

**Ordering:** Positionals and flags are emitted in the **source order** in which they appear in the config. There is no special "positionals trail" rule — a positional written before a flag in the config appears before that flag on the resolved command line. This allows commands that require leading positionals or interleaved positionals (e.g. `git clone <url> --depth 1`, `ffmpeg -i in.mp4 -c:v libx264 out.mp4`) to be expressed directly:

```kdl
git {
    clone-myrepo {
        "clone"
        "https://github.com/me/repo.git"
        depth 1
    }
}
```

Resolves to: `git clone https://github.com/me/repo.git --depth 1`

### 2.7 Defaults and profiles

A command node's children are walked in source order at resolution time. Each child is either:

- A **default argument** (a flag or a positional, as defined in §2.4) — always emitted, regardless of which profile is selected.
- A **profile** (a node with a child block) — emitted only if it is the profile selected on the command line. When emitted, the profile's own children are walked recursively in source order.

This means **profiles are positional slots**: they may be interleaved freely with defaults, and the position at which a profile is written in the config determines where its arguments appear in the resolved command line.

```kdl
some-tool {
    "default-positional"

    profile-a {
        timeout 30
    }

    profile-b {
        timeout 60
    }

    verbose #true

    profile-c {
        timeout 90
    }
}
```

| Command                    | Resolved |
|----------------------------|----------|
| `jig some-tool`            | `some-tool default-positional --verbose` |
| `jig some-tool profile-a`  | `some-tool default-positional --timeout 30 --verbose` |
| `jig some-tool profile-b`  | `some-tool default-positional --timeout 60 --verbose` |
| `jig some-tool profile-c`  | `some-tool default-positional --verbose --timeout 90` |

The `verbose` default is always emitted because it lives outside any profile. Each profile's `timeout` is emitted only when that profile is selected, and at the position where the profile is written.

### 2.8 Merge semantics

When `jig <command> <profile>` is invoked, the resolved argument list is built by a **single source-order walk** of the command node's children:

1. **Walk the command's children in source order.** For each child:
   - If the child is a default argument (flag or positional), emit it.
   - If the child is the **selected** profile, walk its children in source order, emitting each.
   - If the child is some **other** profile, skip it entirely.

This produces an ordered list of candidate arguments. Two transformations then apply:

2. **Flag override (first-occurrence positioning).** If the same flag key appears more than once in the candidate list (which can happen when both defaults and the selected profile contribute a value for that key), all occurrences except the first are removed, and the first occurrence's value is replaced with the value from the **profile** (if the profile contributed a value for that key) or the **default** (if only defaults did).

   In other words: the **position** of the merged flag is the position of its first occurrence in the source-order walk; the **value** is the profile's if present, otherwise the default's.

3. **Suppression.** Any flag whose final resolved value is the boolean `#false` is dropped from the resolved command. This applies regardless of whether the default's value was a boolean, a string, or a number — `#false` is a universal "remove this flag" marker.

Positionals are not subject to override or suppression. They are emitted at the position they were written, in source order, as part of step 1.

#### 2.8.1 First-occurrence positioning examples

```kdl
some-tool {
    timeout 10
    verbose #true

    fast {
        timeout 5
    }
}
```

`jig some-tool fast` → walk produces `[timeout=10, verbose=true, timeout=5]` → first-occurrence collapse with profile value → `[timeout=5, verbose=true]` → resolved: `some-tool --timeout 5 --verbose`

The `--timeout` flag stays at the *default's* position (where `timeout` was first encountered in the walk), but takes the *profile's* value.

```kdl
some-tool {
    fast {
        timeout 5
    }
    timeout 10
    verbose #true
}
```

`jig some-tool fast` → walk produces `[timeout=5, timeout=10, verbose=true]` → first-occurrence collapse → `[timeout=5, verbose=true]` → resolved: `some-tool --timeout 5 --verbose`

Here the profile is written first, so `--timeout` ends up at the profile's position.

#### 2.8.2 Cross-type override examples

The merge rules apply uniformly across value types. A profile may override a default with a different type of value:

```kdl
some-tool {
    xxx "test"
    timeout 30
    verbose #true

    quiet {
        xxx #false       // suppress --xxx entirely
        timeout #false   // suppress --timeout entirely
        verbose #false   // suppress --verbose entirely
    }

    loud {
        xxx "verbose-mode"   // override string with different string
        timeout 5            // override number with different number
    }

    flag-form {
        xxx #true        // convert "--xxx test" default into bare "--xxx"
    }
}
```

| Command                       | Resolved |
|-------------------------------|----------|
| `jig some-tool`               | `some-tool --xxx test --timeout 30 --verbose` |
| `jig some-tool quiet`         | `some-tool` |
| `jig some-tool loud`          | `some-tool --xxx verbose-mode --timeout 5 --verbose` |
| `jig some-tool flag-form`     | `some-tool --xxx --timeout 30 --verbose` |

### 2.9 Constraints and errors

- A command name may appear more than once across the file. If a command name appears more than once, **every** occurrence must declare an alias, and those aliases must all be distinct (per the alias uniqueness rule below). A duplicated command name is **not** a valid lookup key — invocations of that command must use one of its aliases. A command name that appears exactly once may be invoked either by that name or (if present) by its alias.
- A command's alias (if present) must be unique across the file. Duplicate aliases are a parse error.
- An alias may not collide with any non-duplicated command name in the file. A command may declare an alias equal to its own name (e.g. `foo "foo" {...}`) when that name appears exactly once; this is harmless redundancy and not a collision. A duplicated command name may not be used as an alias anywhere (including as one of its own occurrences' alias), since that would silently shadow the bare-name ambiguity.
- Within a command, profile names must be unique. Duplicates are a parse error.
- Within a single scope (a command's defaults, or a single profile's body), each flag key must appear at most once **in its resolved CLI form** — i.e. after the §2.5 prefix synthesis. For example, `host "a"` and `--host "b"` both resolve to `--host` and are duplicates. Duplicate flag keys within the same scope are a parse error. (Positionals naturally have no key and may repeat freely.)
- Command names, aliases, and profile names must not start with `-` (would be ambiguous with `jig`'s own flags).

A profile (a node with children) and a default argument (a node without children) may share the same identifier within a command. They are structurally distinct in the KDL source and play different roles at resolution time, so no collision exists.

## 3. Command-Line Interface

### 3.1 Invocation grammar

```
jig [JIG_FLAGS]... <command-or-alias> [profile] [PASSTHROUGH]...
```

- `JIG_FLAGS`: flags consumed by `jig` itself. Must appear **before** the command name.
- `<command-or-alias>`: matches either a command name or an alias from the config. Lookup checks command names first, then aliases (per §4 step 3). When a command name appears more than once, the bare name is not a valid lookup key — using it produces an "ambiguous command name" error that lists the available aliases.
- `[profile]`: optional profile name. If omitted, only command defaults are used.
- `[PASSTHROUGH]`: any further arguments are appended verbatim to the resolved command line.

The first non-flag argument is treated as the command/alias. Everything from there onward (including args that look like flags) is consumed positionally or as pass-through.

### 3.2 Pass-through

All arguments after the profile (or after the command, if no profile) are appended to the resolved command line, unmodified, in order.

A literal `--` token, if present in the pass-through region, is **passed through verbatim** (not stripped). This allows the target command to use `--` as its own separator if it needs to.

```
jig serve qwen-coder -x --abc -y
  → llama-server <resolved-args> -x --abc -y

jig serve qwen-coder -- --abc
  → llama-server <resolved-args> -- --abc
```

### 3.3 Pass-through placement

Pass-through args are appended at the **very end** of the resolved command line, after all arguments that came from the config:

```
[resolved-arguments-from-defaults-and-profile-in-source-order] [pass-through]
```

Rationale: pass-through tokens are user additions on top of the resolved command, applied at runtime. Trailing them is the standard wrapper-tool convention (matches `env`, `timeout`, `nohup`, etc.) and avoids any interaction with the config's source-order rules.

### 3.4 jig's own flags

| Flag                  | Description |
|-----------------------|-------------|
| `-n`, `--dry-run`     | Print the resolved command (shell-quoted) to stdout and exit 0 without executing. |
| `--config <PATH>`     | Use `<PATH>` instead of looking for `jig.kdl` / `.jig.kdl` in CWD. |
| `-l`, `--list`        | List all configured commands, aliases, and profiles. Exits 0. |
| `--completions <SHELL>` | Generate a shell completion script for `<SHELL>` (`zsh`, `bash`, `fish`) and write it to stdout. Exits 0. Hidden from `--help`. |
| `-h`, `--help`        | Print help and exit. |
| `-V`, `--version`     | Print version and exit. |

Long-form `--dry-run` is canonical; `-n` mirrors `make`/`ninja` convention.

The `--completions` flag emits a completion script that completes `jig`'s own flags and dispatches to `jig --list-commands` and `jig --list-profiles <COMMAND>` to enumerate command names, aliases, and profile names from the local `jig.kdl` at completion time. The shell scripts forward an explicit `--config <PATH>` from the user's command line so candidates always reflect the chosen config.

The `--list-commands` and `--list-profiles <COMMAND>` flags are hidden completion-only flags. They print one candidate per line on stdout and never produce stderr. Any failure to load or validate the config results in empty stdout and exit code `0`, so completion never breaks mid-tab. A bare command name that appears more than once in the config is excluded from `--list-commands` output (it is not a valid lookup key per §2.9 / §4), and `--list-profiles` for such a name produces no output — the user must invoke via an alias.

### 3.5 Exit codes

`jig` follows the wrapper-tool exit code convention used by `env(1)`, `timeout(1)`, `nohup(1)`, and similar utilities. This avoids collision with codes commonly returned by target commands (especially `1` and `2`).

| Code     | Meaning |
|----------|---------|
| 0        | Successful resolution and execution (or `--dry-run` / `--list` / `--completions` / `--help` / `--version`). |
| 125      | `jig` itself failed: missing config, parse error, constraint violation, unknown command/profile/alias, bad CLI usage. |
| 126      | The resolved command was found but is not executable (e.g. permission denied, not a regular file). |
| 127      | The resolved command was not found in `$PATH`. |
| anything else | Exit code propagated verbatim from the executed command (including signal-related codes like 128+N). |

Rationale: target commands very commonly use exit codes `1` (generic failure) and `2` (e.g. `grep` no-match, GNU usage errors). If `jig` used these for its own errors, callers and shell scripts could not distinguish a `jig` failure from a target-command failure. The 125–127 range is a long-standing wrapper-tool convention specifically reserved for this purpose.

### 3.6 Execution semantics

- `jig` resolves the target command via `$PATH` (or treats it as a path if it contains a path separator), spawns it as a child process, and waits for it to exit.
- `jig` exits with the child's exit status, except where overridden by the wrapper-tool exit codes in §3.5.
- Standard streams (stdin, stdout, stderr) are inherited from `jig` to the child.
- Signals (SIGINT, SIGTERM) delivered to `jig` reach the child naturally because the child is in the same process group; no explicit signal forwarding is performed by `jig` for v1.

Implementation note: `std::process::Command::status()` is the appropriate Rust API. `execvp`-style replacement (`exec`) was considered but rejected because it would prevent `jig` from translating non-zero child statuses through the §3.5 exit-code conventions and would lose the ability to perform any post-execution work in the future.

## 4. Resolution Algorithm

Given `jig <name> [profile] [passthrough...]`:

1. Locate config file (§2.1). Error if not found.
2. Parse KDL. Error on syntax errors or constraint violations (§2.9).
3. Look up `<name>`:
   - First, count commands whose name equals `<name>`. If exactly one, that's the match. If more than one, error: "command name `<name>` is ambiguous"; the help lists the aliases of the duplicated entries.
   - Otherwise, find the unique command (if any) whose alias equals `<name>`. If found, that's the match.
   - Otherwise, error: "unknown command or alias: `<name>`".
4. If `[profile]` is provided:
   - Look up profile within the matched command. If not found, error: "unknown profile <profile> for command <command>".
5. Build resolved argument list per §2.8:
   - Walk the command's children in source order. For each child:
     - If it is a default argument, append it to the candidate list.
     - If it is the selected profile, walk its children in source order, appending each to the candidate list.
     - If it is some other profile, skip.
   - Apply flag override using first-occurrence positioning: collapse duplicate flag keys to a single entry at the position of the first occurrence, with the profile's value taking precedence over the default's where both contributed.
   - Drop any flags whose resolved value is the boolean `#false`.
6. For each remaining flag, format per the flag prefix rules (§2.5). Boolean `#true` flags emit only the flag key (no accompanying value). Positionals emit their literal value.
7. Append pass-through args at the end (§3.3).
8. If `--dry-run`: print shell-quoted command line; exit 0.
9. Otherwise: execute the resolved command. Exit with the child's exit status.

## 5. Worked Examples

### 5.1 llama-server

Config:

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

Invocations:

| Command                              | Resolved |
|--------------------------------------|----------|
| `jig serve`                          | `llama-server --host 0.0.0.0 --port 8090 -c 32768 --flash-attn` |
| `jig serve qwen-coder`               | `llama-server --host 0.0.0.0 --port 8090 -c 32768 --flash-attn -m /models/qwen-coder.gguf -ngl 999 -ts 0.5,0.5` |
| `jig serve llama3`                   | `llama-server --host 0.0.0.0 --port 8091 -c 32768 --flash-attn -m /models/llama3.gguf` |
| `jig llama-server qwen-coder`        | (same as `jig serve qwen-coder`) |
| `jig --dry-run serve qwen-coder`     | (prints resolved command, doesn't execute) |
| `jig serve qwen-coder --verbose`     | `llama-server <args-from-profile> --verbose` (pass-through) |

### 5.2 Override and suppression

```kdl
some-tool {
    verbose #true
    timeout 30
    log-file "/var/log/some-tool.log"

    quiet-profile {
        verbose #false
        timeout 5
    }

    no-log {
        log-file #false
    }
}
```

| Command                          | Resolved |
|----------------------------------|----------|
| `jig some-tool`                  | `some-tool --verbose --timeout 30 --log-file /var/log/some-tool.log` |
| `jig some-tool quiet-profile`    | `some-tool --timeout 5 --log-file /var/log/some-tool.log` |
| `jig some-tool no-log`           | `some-tool --verbose --timeout 30` |

Note in particular `no-log`: a profile uses `log-file #false` to suppress a default flag whose original value was a string, not a boolean. `#false` is a universal suppression marker (see §2.8.2).

### 5.3 Positionals in source order

```kdl
rsync "sync" {
    archive #true
    verbose #true

    backup {
        "/source/"
        "user@host:/dest/"
    }
}
```

`jig sync backup` → `rsync --archive --verbose /source/ user@host:/dest/`

The positionals appear after the defaults' flags simply because they are written in the profile block (which follows the defaults block in source order), not because of any "positionals trail" rule.

### 5.3.1 Interleaved positionals

When positionals must appear before or between flags, write them in that order:

```kdl
ffmpeg "transcode" {
    h264 {
        i "input.mp4"
        -c:v "libx264"
        "output.mp4"
    }
}

git {
    clone-myrepo {
        "clone"
        "https://github.com/me/repo.git"
        depth 1
    }
}
```

| Command                       | Resolved |
|-------------------------------|----------|
| `jig transcode h264`          | `ffmpeg -i input.mp4 -c:v libx264 output.mp4` |
| `jig git clone-myrepo`        | `git clone https://github.com/me/repo.git --depth 1` |

### 5.4 String value that looks like a boolean

Some tools take literal `true`/`false` as argument values rather than as bare flags:

```kdl
mytool {
    enabled-true {
        enabled "true"
    }
    enabled-false {
        enabled "false"
    }
    enabled-flag {
        enabled #true
    }
}
```

| Command                          | Resolved |
|----------------------------------|----------|
| `jig mytool enabled-true`        | `mytool --enabled true` |
| `jig mytool enabled-false`       | `mytool --enabled false` |
| `jig mytool enabled-flag`        | `mytool --enabled` |

### 5.5 Same binary, multiple profile sets

Two top-level entries may share a command name when each declares a distinct alias. The shared name is then no longer a valid lookup key — invocation must go through one of the aliases:

```kdl
llama-server "serve-coder" {
    host "0.0.0.0"
    port 8090
    qwen-coder {
        m "/models/qwen-coder.gguf"
    }
}

llama-server "serve-chat" {
    host "0.0.0.0"
    port 8091
    llama3 {
        m "/models/llama3.gguf"
    }
}
```

| Command                              | Resolved |
|--------------------------------------|----------|
| `jig serve-coder qwen-coder`         | `llama-server --host 0.0.0.0 --port 8090 -m /models/qwen-coder.gguf` |
| `jig serve-chat llama3`              | `llama-server --host 0.0.0.0 --port 8091 -m /models/llama3.gguf` |
| `jig llama-server`                   | error: ambiguous command name; use one of `serve-coder`, `serve-chat` |

## 6. Out of Scope for v1

The following are deliberately deferred. None of them are precluded by the v1 design.

- Parent-directory config traversal.
- Global / user-level config (`~/.config/jig/`).
- Environment variable interpolation in values.
- Templating, computed values, or includes.
- Profile inheritance between profiles (only command-level defaults).
- Multiple aliases per command.
- Multiple occurrences of the same profile within a command.
- Subcommand chains beyond what fits in a quoted command name.
- `--print` variants (e.g. argv-array form vs shell-quoted form).
- Validation of resolved commands beyond `Command::spawn` failures (e.g. proactive existence/executability checks before launching).
- Environment variable definitions in config (delegated to a future version).
- Working-directory annotations.

## 7. Resolved Design Decisions

These were open during spec drafting and have been resolved:

### 7.1 `--list` output format

Human-readable text only for v1. No JSON or other machine-readable form. May be added later if a real need emerges.

The output should be readable enough to grep and eyeball, but is not promised to be stable for scripting. Suggested format (non-normative, for implementation guidance):

```
llama-server (alias: serve)
  default-args: --host 0.0.0.0 --port 8090 -c 32768 --flash-attn
  profiles:
    qwen-coder
    llama3

rsync (alias: sync)
  default-args: --archive --verbose
  profiles:
    backup
```

### 7.2 `--dry-run` output format

Output the resolved command as a **single line, properly shell-quoted**, such that the line can be copy-pasted into a POSIX shell and executed with the exact same effect as omitting `--dry-run` would have produced.

This means:
- Arguments containing spaces, glob characters, quotes, or other shell-significant characters must be quoted (typically with single quotes, with embedded single quotes escaped using the standard `'\''` idiom).
- Arguments containing no shell-significant characters may be emitted unquoted for readability.
- The output goes to stdout. No trailing prompt, no leading `$`, no log decoration.

Example:
```
$ jig --dry-run serve qwen-coder
llama-server --host 0.0.0.0 --port 8090 -c 32768 --flash-attn -m /models/qwen-coder.gguf -ngl 999 -ts 0.5,0.5
```

Argv-style (one argument per line) is **not** offered in v1. If users need to inspect quoting, they can pipe the dry-run output to a shell parser, or we can revisit later.

### 7.3 No `--show` / `--explain` flag

Not provided. The combination of `--list` (to see what's defined) and `--dry-run` (to see what would be executed) is sufficient for the realistic debugging needs of a tool this small. A dedicated explanation mode that traces "default A applied, then profile overrode B with C" is overkill for v1 and probably for any version.

### 7.4 Error reporting

Errors from `jig` should be **maximally informative**. The goal is for a user to understand and fix the issue from the error message alone, without needing to read the spec.

Error messages must include, where applicable:

- **The config file path** that was loaded (so the user knows which file to edit).
- **Line and column numbers** for parse errors and constraint violations.
- **A snippet of the offending source** (the line itself, with a caret or similar marker pointing at the column).
- **The semantic problem in plain English**, not just a parser-internal error code (e.g. "alias 'serve' is defined on both 'llama-server' (line 12) and 'gemma-server' (line 47)" — not "duplicate key").
- **Cross-references to the related location** when an error involves two places (e.g. the original definition site of a colliding alias).
- **A suggested fix** when one is obvious (e.g. "did you mean 'qwen-coder'?" for a profile-not-found error, using nearest-name matching).

For KDL parse errors specifically, the underlying parser's diagnostic output (the `kdl` Rust crate produces `miette`-style diagnostics with source spans) should be surfaced rather than discarded, prefixed with the config file path.

Errors that originate from the CLI invocation rather than from the config file (e.g. an unknown command, alias, or profile typed by the user) do **not** carry source spans, since the offending text is not in the config. These errors render as a message plus any relevant alternatives and a `hint:` line — no `-->` source-span block.

Examples of acceptable error output:

```
error: config file not found
  searched: ./jig.kdl, ./.jig.kdl
  in directory: /home/user/project
  hint: create a jig.kdl file with at least one command definition
```

```
error: unknown profile 'qwen-codr' for command 'llama-server'
  available profiles: qwen-coder, llama3
  hint: did you mean 'qwen-coder'?
```

```
error: alias 'serve' is defined more than once
  --> jig.kdl:12:14
   |
12 | llama-server "serve" {
   |              ^^^^^^^ first defined here
   |
  --> jig.kdl:47:14
   |
47 | gemma-server "serve" {
   |              ^^^^^^^ also defined here
   |
  hint: each alias may be used by at most one command
```

Implementation note: leverage the `miette` ecosystem (already used by the `kdl` crate) for span-aware diagnostics.
