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
- Environment variable expansion or templating.

## 2. Configuration File

### 2.1 Location

`jig` looks for a configuration file by starting in the current working directory and walking upward through its ancestors. Within each directory it checks, in order:

1. `jig.kdl` (preferred, visible)
2. `.jig.kdl` (hidden fallback)

The first directory that contains either file ends the search; within that directory, `jig.kdl` is used and `.jig.kdl` is silently ignored when both are present. No merging is performed across directories — only the nearest configuration is loaded.

The upward walk is bounded by `$HOME`: if `$HOME` is an ancestor of the current working directory, the walk stops after checking `$HOME` and does not cross above it. Otherwise (including when `$HOME` is unset, or when the current working directory is outside `$HOME`) the walk continues up to the filesystem root.

If no configuration is found anywhere in the search range, `jig` exits with an error that reports the file names searched, the starting directory, and the last directory actually checked.

When the `--config <PATH>` flag is provided, the explicit path is used directly and the upward search is skipped entirely. The path may be absolute or relative (relative paths are resolved against the CWD as usual).

`jig` does not change its working directory when the configuration is found in an ancestor: the resolved command is still executed from the user's CWD.

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

#### 2.4.3 The `#null` placeholder

A flag whose value is the KDL keyword `#null` is a **position-only placeholder**:

- It declares the flag at this source position but contributes no value.
- It is never emitted on the resolved command line (no `-a #null` literal in argv).
- It does not suppress anything (unlike `#false`); a `#null` at any tier does not clear other tiers' entries for the same key, and a profile-side `#null` does not override a default's value.
- It is not a survivor for mode selection (a `#null` in repeat-mode candidates does not push the side over the multiplicity threshold).
- Its **source position** is retained for the first-occurrence rule (§2.8 step 2.4 / §2.8.5 step 4). When the per-key merge emits in single mode, the merged occurrence sits at the earliest source index among the surviving unmarked candidates and any `#null` ghosts for the same key.

Typical use: declare command-level documentation — list every flag the command supports at canonical positions — and let profiles supply the actual values:

```kdl
some-tool {
    m #null            // every profile must supply a model path
    host "0.0.0.0"

    fast { m "/m1" }
    slow { m "/m2" }
}
```

`jig some-tool fast` → `some-tool -m /m1 --host 0.0.0.0`. The `-m` emits at the `#null` slot (idx 0), before `--host`. Without a profile, `jig some-tool` → `some-tool --host 0.0.0.0` — the placeholder has no value to emit.

The `+` append marker (§2.5 rule 0) on a `#null` is rejected as a parse error: `#null` has nothing to emit separately, so combining it with the marker is meaningless. Quoted `"#null"` is a literal string value (the four characters `#null`), not the keyword — same convention as `"true"` vs `#true` in §2.4.1.

### 2.5 Flag prefix rules

The mapping from key name to CLI flag is:

0. If the key starts with `+`, the leading `+` is stripped and the occurrence is marked as an **explicit append** (see §2.8). The remaining text is then processed by rules 1–3 to determine the resolved CLI form. The marker applies only to flag nodes (those with a value); on a positional node the leading `+` is part of the literal value.
1. If the (post-rule-0) key starts with `-` or `--`, it is passed verbatim.
2. Otherwise, if the key is exactly **1 character**, prefix with `-` (single dash).
3. Otherwise (2+ characters), prefix with `--` (double dash).

Rule 0 examples:

| Source             | Marked? | Resolved CLI |
|--------------------|---------|--------------|
| `+I "/p"`          | yes     | `-I /p`      |
| `+host "x"`        | yes     | `--host x`   |
| `+-ngl 999`        | yes     | `-ngl 999`   |
| `+--explicit "v"`  | yes     | `--explicit v` |

A bare `+` with no key text after it is a parse error. A leading `+` on a command, alias, or profile name is also a parse error (§2.9), since names of any kind cannot start with `+`. The marker is also rejected on a `#null` placeholder (§2.4.3): `#null` has nothing to emit separately.

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

A profile may declare a parent via `extends="<parent-profile>"`. Selecting an inheriting profile activates every profile in its inheritance chain — each one's body emits at its own source position. See §2.8.5.

### 2.8 Merge semantics

When `jig <command> <profile>` is invoked, the resolved argument list is built in three steps:

1. **Walk the command's children in source order.** For each child:
   - If the child is a default argument (flag or positional), emit it as a candidate.
   - If the child is the **selected** profile, walk its children in source order, emitting each as a candidate from the profile side.
   - If the child is some **other** profile, skip it entirely.

   This produces an ordered list of candidate arguments. Each flag candidate carries its value, its `+` marker (set or unset, per §2.5 rule 0), and whether it came from defaults or the selected profile.

2. **Per-key resolution.** Group flag candidates by their resolved CLI form (after §2.5 prefix synthesis), then for each key apply this rule:

   1. **Suppression.** If any profile-side candidate for the key has value `#false` (regardless of whether it carries the `+` marker), drop *every* default-side candidate for the key and drop the `#false` profile entries; profile-side candidates with non-`#false` values are kept regardless of marker. Otherwise, drop just those default-side or profile-side candidates whose value is `#false`. (`#false` is a universal "remove this flag" marker; profile-side use additionally clears any defaults of the same key.)
   2. **Marker partition.** Among the survivors, separate `+`-marked entries from unmarked entries.
   3. **Marked entries always emit.** Each `+`-marked candidate emits at its own source position with its own value. Marked entries do not collapse with anything.
   4. **Unmarked entries pick a mode.** Let `D_unmarked` and `P_unmarked` be the surviving unmarked entries on the default and profile sides respectively.
      - If `|D_unmarked| ≤ 1` *and* `|P_unmarked| ≤ 1`: **single mode** (v1 behavior). Emit at most one occurrence, at the **first-occurrence source position** (the earliest source index of an unmarked survivor or a `#null` ghost (§2.4.3) for this key), with the profile's value if `P_unmarked` is non-empty, otherwise the default's.
      - Otherwise (either side has two or more unmarked entries): **repeat mode**. Every unmarked occurrence emits at its own source position. No collapsing.

3. **Assemble.** Walk the candidate list in source order. Positionals always emit at their own position; flag candidates emit iff the per-key resolution kept them.

Positionals are not subject to override or suppression. They are emitted at the position they were written, in source order, as part of step 1.

The single-mode case (≤ 1 unmarked occurrence on each side) is the v1 idiom: a default sets a flag and a profile optionally overrides it. Repeat mode is for tools that legitimately accept the same flag more than once (e.g. `gcc -I /a -I /b`, `curl --header A --header B`, count flags like `-v -v -v`). The `+` marker is the explicit knob for the case the multiplicity rule cannot disambiguate: a single default plus a single profile entry that should *add* rather than *replace* (§2.8.4).

#### 2.8.1 First-occurrence positioning (single mode)

When both sides contribute at most one unmarked occurrence, the merged occurrence sits at the position of its first appearance in the walk:

```kdl
some-tool {
    timeout 10
    verbose #true

    fast {
        timeout 5
    }
}
```

`jig some-tool fast` → candidates `[timeout=10 (default), verbose=true (default), timeout=5 (profile)]` → single mode for `--timeout` → emit at default's position with profile's value → `some-tool --timeout 5 --verbose`.

```kdl
some-tool {
    fast {
        timeout 5
    }
    timeout 10
    verbose #true
}
```

`jig some-tool fast` → single mode → `--timeout` emits at the profile's position (the first one walked) with the profile's value → `some-tool --timeout 5 --verbose`.

#### 2.8.2 Cross-type override examples (single mode)

The single-mode rule applies uniformly across value types. A profile may override a default with a different type of value:

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

#### 2.8.3 Repeat mode

When either side contributes two or more unmarked occurrences of the same key, every unmarked occurrence emits at its own source position:

```kdl
gcc {
    I "/usr/include"
    I "/opt/include"

    project-a {
        I "/proj/a/include"
    }
}
```

| Command                          | Resolved |
|----------------------------------|----------|
| `jig gcc`                        | `gcc -I /usr/include -I /opt/include` |
| `jig gcc project-a`              | `gcc -I /usr/include -I /opt/include -I /proj/a/include` |

Count flags fall out of the same rule:

```kdl
some-tool {
    v #true
    v #true
    v #true
}
```

`jig some-tool` → `some-tool -v -v -v`.

A profile can use `#false` to clear all defaults of the same key — the suppression rule above ensures profile-side `#false` wipes the defaults' list. To replace a default list with a different one, write `K #false` followed by the new entries:

```kdl
gcc {
    I "/default"

    bare    { I #false }
    custom  {
        I #false
        I "/mine"
    }
}
```

| Command            | Resolved |
|--------------------|----------|
| `jig gcc`          | `gcc -I /default` |
| `jig gcc bare`     | `gcc` |
| `jig gcc custom`   | `gcc -I /mine` |

#### 2.8.4 Explicit append marker

The multiplicity rule in §2.8 step 2.4 cannot disambiguate one specific case: a single unmarked default plus a single unmarked profile entry that the user wants to *add* rather than *replace*. The `+` marker (§2.5 rule 0) is the explicit knob for that case. A `+`-prefixed flag always emits at its own position and never collapses with unmarked occurrences:

```kdl
gcc {
    I "/usr/include"

    proj-extras {
        +I "/proj/include"
    }
}
```

`jig gcc proj-extras` → `gcc -I /usr/include -I /proj/include`.

Without the `+`, the same shape would be single mode and `--I /proj/include` would replace the default:

```kdl
gcc {
    I "/usr/include"

    proj-replace {
        I "/proj/include"
    }
}
```

`jig gcc proj-replace` → `gcc -I /proj/include`.

The marker can mix with unmarked entries inside a single profile body:

```kdl
gcc {
    I "/usr/include"

    mixed {
        I "/replace"      // unmarked → single-mode override
        +I "/extra"       // marked → emits separately
    }
}
```

`jig gcc mixed` → `gcc -I /replace -I /extra`.

`+I #false` and unmarked `I #false` apply suppression identically: the `#false` clears defaults regardless of marker (§2.8 step 2.1).

#### 2.8.5 Profile inheritance

A profile node may carry a `extends="<parent-profile>"` KDL property naming another profile within the same command:

```kdl
llama-server "serve" {
    host "0.0.0.0"
    port 8090

    qwen-coder {
        m "/models/qwen-coder.gguf"
        -ngl 999
    }

    qwen-coder-large extends="qwen-coder" {
        m "/models/qwen-coder-large.gguf"
    }
}
```

When the selected profile inherits from a parent (and the parent may itself inherit from a grandparent, and so on), every profile in the chain is "activated": each one's body emits at its own source position when the leaf is selected. The chain extends upward from the selected leaf to a root profile that carries no `extends`.

`jig serve qwen-coder-large` resolves to `llama-server --host 0.0.0.0 --port 8090 -m /models/qwen-coder-large.gguf -ngl 999`: defaults pass through, `qwen-coder`'s `-ngl 999` is inherited, and `qwen-coder-large` overrides `-m`.

Restrictions in v1:

- `extends` references are scoped to the same command. Cross-command inheritance is not supported.
- A profile inherits from at most one parent. Diamond / multi-parent inheritance is not supported.
- The `extends` graph must be acyclic. Cycles are rejected at validation time with a diagnostic that names every profile on the cycle.
- Forward declarations are allowed: a child may textually precede its parent within the command body. Validation runs after the whole config is parsed.

The merge algorithm in §2.8 generalises directly. Replace the two-tier "defaults vs profile" with an N-tier cascade: tier 0 is defaults, tier 1 is the chain's root ancestor, …, tier N is the selected leaf. The per-key rules in §2.8 step 2 become:

1. **Suppression.** Let `T = max { tier of any #false survivor with tier > 0 }`. If `T` exists, drop every entry whose tier is strictly less than `T`, and drop every `#false` entry regardless of tier. If `T` does not exist, drop only the `#false` entries themselves.
2. **Marker partition** is unchanged: `+`-marked entries always emit at their own source position with their own value.
3. **Mode selection.** Single mode iff *every* tier contributes ≤ 1 unmarked survivor; otherwise repeat mode.
4. **Single-mode** emits one occurrence at the **earliest source index** among unmarked survivors and `#null` ghosts (§2.4.3) for this key, with the **highest-tier** value among the unmarked survivors. A `#null` ghost contributes its source position but never its value; a ghost at a tier dropped by step 1's `T`-cascade is dropped along with the rest of its tier. With no inheritance and no `#null` interactions this collapses to §2.8.1's first-occurrence rule.
5. **Repeat mode** emits each unmarked survivor at its own source index, unchanged from §2.8 step 2.4.

Env-var resolution (§2.11) generalises in the same way: walk tiers descending (leaf → … → defaults); the first tier with an outcome for the name wins. The first-occurrence position rule continues to use the ascending walk so `--list` and `--dry-run` order stay deterministic.

A worked example with a three-level chain:

```kdl
foo {
    grand { x "from-grand" }
    parent extends="grand" { x "from-parent" }
    child extends="parent" { x "from-child" }
}
```

| Command            | Resolved |
|--------------------|----------|
| `jig foo grand`    | `foo -x from-grand` |
| `jig foo parent`   | `foo -x from-parent` |
| `jig foo child`    | `foo -x from-child` |

Each invocation activates a different leaf and so a different chain; the highest-tier value wins.

### 2.9 Constraints and errors

- A command name may appear more than once across the file. If a command name appears more than once, **every** occurrence must declare an alias, and those aliases must all be distinct (per the alias uniqueness rule below). A duplicated command name is **not** a valid lookup key — invocations of that command must use one of its aliases. A command name that appears exactly once may be invoked either by that name or (if present) by its alias.
- A command's alias (if present) must be unique across the file. Duplicate aliases are a parse error.
- An alias may not collide with any non-duplicated command name in the file. A command may declare an alias equal to its own name (e.g. `foo "foo" {...}`) when that name appears exactly once; this is harmless redundancy and not a collision. A duplicated command name may not be used as an alias anywhere (including as one of its own occurrences' alias), since that would silently shadow the bare-name ambiguity.
- Within a command, profile names must be unique. Duplicates are a parse error.
- A profile's `extends="<parent>"` (§2.8.5) must name another profile within the same command. An unknown parent is a parse error.
- The per-command `extends` graph must be acyclic. Cycles are a parse error; the diagnostic names every profile on the cycle.
- A profile node may only carry the named property `extends`; any other KDL property on a profile node is a parse error. Profile nodes may not carry positional values.
- Repeated flag keys within a single scope are **allowed**. The merge algorithm in §2.8 picks single-mode vs repeat-mode resolution per key based on the multiplicity of unmarked occurrences across the default and selected-profile sides; the `+` marker (§2.5 rule 0) opts an occurrence out of v1's first-occurrence collapse. (Positionals have no key and may repeat freely.)
- Command names, aliases, and profile names must not start with `-` (would be ambiguous with `jig`'s own flags) or `+` (reserved for the explicit append marker on flag keys, §2.5).
- Environment-variable names (the identifier on a `(env)`-annotated node, §2.10) must match the POSIX-portable pattern `[A-Za-z_][A-Za-z0-9_]*`. Within a single scope (the defaults of one command, or the body of one profile) each env-var name must be unique; cross-scope occurrences (defaults plus a profile) are how overrides are expressed and remain allowed.
- Type annotations on KDL nodes are recognized only where they are explicitly defined. The only annotation defined in v1 is `(env)` (§2.10), which applies to argument-shaped nodes (no children, exactly one value or `#false`) inside a command body or a profile body. Any other annotation, or `(env)` outside the contexts above, is a parse error.
- The `cwd="<path>"` property (§2.12) is allowed only on a command node or a profile node, alongside any other property that node permits (e.g. `extends=` on profiles). Its value must be a non-empty string; `#true`, `#false`, `#null`, numeric values, and the empty string are parse errors. A given node may carry at most one `cwd=` property.

A profile (a node with children) and a default argument (a node without children) may share the same identifier within a command. They are structurally distinct in the KDL source and play different roles at resolution time, so no collision exists.

### 2.10 Environment variables

A KDL node bearing the `(env)` type annotation declares an environment-variable contribution rather than a CLI argument:

```kdl
llama-server "serve" {
    host "0.0.0.0"
    (env)OLLAMA_HOST "0.0.0.0"
    (env)CUDA_VISIBLE_DEVICES "0,1"

    qwen-coder {
        m "/models/qwen-coder.gguf"
        (env)CUDA_VISIBLE_DEVICES "0"     // override
        (env)OLLAMA_HOST #false           // unset (env_remove)
        (env)EXTRA_VAR "yes"              // new
    }
}
```

Rules:

- Env declarations may appear in a command's defaults block or in a profile body, sibling to flag, positional, and profile nodes. They must not appear at top level (where a node is a command), and must not appear on a node with a child block (env vars do not have profile-like bodies).
- The annotated node must carry **exactly one value**. The value may be a string, integer, or float (emitted via the same source-representation preservation as flag values, §2.4 / §7.2), or the literal boolean `#false` to mean "unset this variable on the child" (calls into `env_remove(NAME)`). The literal `#true` is not allowed (env vars require a value); a no-value form is not allowed; `#null` is not allowed.
- The `+` explicit-append marker (§2.5 rule 0) is **not** allowed on `(env)` nodes.
- The env-var name must match `[A-Za-z_][A-Za-z0-9_]*` (§2.9).

Env-var contributions are not arguments and never appear on the resolved argv. They are applied to the spawned child via the merge in §2.11 and the exec rules in §3.6.

### 2.11 Env-var merge semantics

When `jig <command> [profile]` is invoked, env-var contributions are resolved on a parallel channel from flags/positionals:

1. **Walk** the matched command's defaults in source order, then (if a profile was selected) the selected profile's body in source order. Each `(env)` node contributes one candidate `(NAME, value-or-Unset, side)`, where `side ∈ {default, profile}`.
2. **Per-name resolution.** For each distinct `NAME` in the candidate list, decide one outcome:
   - If any profile-side candidate for `NAME` is `Unset` (i.e. `#false`): the resolved outcome is **unset**.
   - Else if any profile-side candidate for `NAME` is `Set(value)`: the resolved outcome is **set to that value**.
   - Else if any default-side candidate for `NAME` is `Unset`: the resolved outcome is **unset**.
   - Else (only default-side `Set`): the resolved outcome is **set to that value**.
3. **Output position.** Each resolved outcome is emitted at the **first occurrence** of `NAME` in the walk. This is what determines the order env vars appear in `--list` (§7.1) and `--dry-run` (§7.2 / §3.4).

There is no repeat mode and no `+` marker for env vars: each name has a single resolved outcome (set to one value, or unset), because POSIX assigns one value per env-var name. Per-scope uniqueness (§2.9) prevents the multiplicity that motivates those mechanisms for flags.

The child process inherits `jig`'s own environment by default; the resolved outcomes are applied on top of that inherited environment (§3.6).

### 2.12 Working directory

A command node or profile node may carry a `cwd="<path>"` KDL property that pins the working directory of the spawned child:

```kdl
some-tool cwd="/abs/path" {
    host "0.0.0.0"

    fast cwd="src" {
        m "./model.gguf"
    }

    plain {
        m "./other.gguf"
    }
}
```

The property is a directive *about* the spawn, not an argv contribution: it does not appear on the resolved command line and is not subject to the per-key merge in §2.8.

#### 2.12.1 Placement and value

- `cwd=` is allowed on a top-level command node and on a profile node. It is rejected on any other node (default-args, positionals, `(env)` declarations).
- The value must be a non-empty string. `#true`, `#false`, `#null`, integers, floats, and the empty string `""` are parse errors.
- A node may carry at most one `cwd=` property. A duplicate is a parse error.
- On a profile node, `cwd=` is allowed alongside `extends=`.
- There is no suppression form in v1 (no `cwd=#false`). Once a command-level `cwd=` is set, every selection of that command runs from somewhere; a profile may *replace* the directory but cannot opt back into the user's CWD. If this turns out to matter, it can be added post-v1.

#### 2.12.2 Path resolution

- **Absolute** paths are used as written.
- **Relative** paths are resolved against the directory containing the loaded config file (the parent of `jig.kdl` / `.jig.kdl`, or the parent of an explicit `--config <PATH>`). The user's current working directory is not consulted.
- The shorthand `cwd="."` therefore means "run from the config-file directory."
- No tilde or environment-variable expansion is performed; symlinks and `..` segments are left to the OS to interpret at `chdir(2)` time.

This anchor choice makes `jig.kdl` portable: a config that says `cwd="."` (or any path relative to the config) behaves the same whether the user invoked `jig` from the project root or a deep subdirectory, which is the whole reason for letting the config file pin the directory.

#### 2.12.3 Effective cwd

For a given invocation, the effective working directory is selected by:

1. If a profile is selected, walk the `extends` chain from leaf to root and use the `cwd=` of the first profile that has one.
2. Otherwise, use the command's `cwd=` if present.
3. Otherwise no `chdir` is performed; the child inherits `jig`'s working directory (the pre-v0.8 behavior).

The leaf-wins rule mirrors §2.8.5: a `cwd=` on a child profile overrides one on its parent. Because each tier supplies at most one `cwd=` value, the merge degenerates trivially to "highest tier wins, no positioning to resolve."

#### 2.12.4 Application

When an effective cwd is determined, `jig` arranges for the spawned child to start in that directory (in `std::process::Command` terms, this is `current_dir(<path>)`). The chdir is local to the child and happens between `fork` and `exec`; `jig` itself does not change its working directory, and the user's shell is unaffected. The env-var contributions (§3.6) and the chdir are independent — neither observes the other in the child.

If the target directory does not exist or cannot be entered, the spawn fails and `jig` exits 125 (a `jig`-configuration failure per §3.5), with a diagnostic that names the path and the underlying OS error.

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

The token immediately after `<command-or-alias>` occupies the **profile slot**. It is interpreted as follows:

- A literal `--` is **consumed** as a "no profile selected" marker; everything from the next token onward is pass-through.
- A token that does not start with `-` is the profile name (per §2.9 profile names cannot start with `-`).
- Anything else (a hyphen-prefixed token other than `--`) leaves the profile slot empty and is the first pass-through token.

This consumption rule applies only to the profile slot. A `--` that appears later — after a real profile, or after a previously-consumed `--` — sits in the pass-through region and is preserved verbatim per §3.2.

### 3.2 Pass-through

All arguments after the profile (or after the command, if no profile) are appended to the resolved command line, unmodified, in order.

A literal `--` token, if present in the pass-through region, is **passed through verbatim** (not stripped). This allows the target command to use `--` as its own separator if it needs to. The profile-slot rule (§3.1) is the one exception: a `--` written *immediately* after `<command-or-alias>` is consumed there as the "no profile" marker and never reaches the pass-through region. To pass a literal `--` as the first pass-through token when no profile is selected, write `--` twice: the first is consumed by the profile slot, the second is preserved.

```
jig serve qwen-coder -x --abc -y
  → llama-server <resolved-args> -x --abc -y

jig serve qwen-coder -- --abc
  → llama-server <resolved-args> -- --abc

jig serve -- foo
  → llama-server <defaults-only-args> foo

jig serve -- -- foo
  → llama-server <defaults-only-args> -- foo
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
| `-q`, `--quiet`       | Suppress the pre-exec preview line (§3.4.1). No effect on `--dry-run`, `--list`, or any other non-exec path. |
| `--config <PATH>`     | Use `<PATH>` instead of looking for `jig.kdl` / `.jig.kdl` in CWD. |
| `-l`, `--list`        | List all configured commands, aliases, and profiles. Exits 0. |
| `--cat`               | Print the loaded config file (preceded by a `cat <path>` header) to stdout and exit 0. Does not require the file to parse — useful for inspecting a broken config. See §3.4.2. |
| `--completions <SHELL>` | Generate a shell completion script for `<SHELL>` (`zsh`, `bash`, `fish`) and write it to stdout. Exits 0. Hidden from `--help`. |
| `-h`, `--help`        | Print help and exit. |
| `-V`, `--version`     | Print version and exit. |

Long-form `--dry-run` is canonical; `-n` mirrors `make`/`ninja` convention.

#### 3.4.1 Pre-exec preview

Before spawning the resolved command, `jig` writes a single line to **stderr** showing exactly what it is about to run. The line uses the same shell-quoted formatting as `--dry-run` (§7.2), including the leading `env(1)` prefix when env-var contributions are present. A trailing newline follows.

When stderr is a terminal, the line is rendered in bold for visibility (ANSI `\x1b[1m…\x1b[0m`). When stderr is not a terminal (redirected to a file or pipe), the line is emitted in plain text without any escape codes so logs and grep output stay clean.

The preview is suppressed when:

- `-q` / `--quiet` is given, or
- `--dry-run` is given (the resolved line is already printed to stdout), or
- any non-exec path is taken (`--list`, `--completions`, `--help`, `--version`, the hidden `--list-commands` / `--list-profiles`).

The preview is purely informational and is not part of the child process's stderr — it is written by `jig` itself, before the child is spawned. It never affects exit codes, argv, or the child's environment.

#### 3.4.2 Configuration dump

`--cat` resolves the config file using the same discovery rules as every other invocation (§2.1) and prints its raw contents to **stdout**, after writing a single header line of the form:

```
cat <path>
```

to **stderr**. Splitting the streams keeps the stdout pipeline pure: `jig --cat | grep …`, `jig --cat | wc -l`, etc. see only the file body, while the header still appears at the terminal as a one-line orientation cue.

The path is rendered relative to the current working directory when possible (with `..` segments if the config sits in an ancestor), and falls back to the absolute path otherwise — same display convention as the `config:` line at the top of `--list` and `--explain` output. The path is shell-quoted, so the header reads as a runnable `cat` invocation against the loaded file. When stderr is a terminal and `NO_COLOR` is unset, the header is rendered in bold (ANSI `\x1b[1m…\x1b[0m`); otherwise it is plain text — same rule the pre-exec preview (§3.4.1) uses for its stderr line.

The body is written to stdout verbatim — no re-encoding, no comment-stripping, no trailing-newline normalization.

Unlike every other config-consuming flag, `--cat` does not require the file to parse: a file that fails KDL parsing or constraint validation is still dumped. The only failure modes are "no config found" and "config found but could not be read", both of which exit 125 with the standard diagnostics. This makes `--cat` a useful tool for inspecting a broken config without having to first locate it manually.

`--cat` is mutually exclusive with `--list`, `--dry-run`, `--explain`, and `--completions`.

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
- The child process inherits `jig`'s own environment, then any env-var outcomes from §2.11 are applied on top: a resolved "set" calls `env(NAME, value)` on the child's `Command`, and a resolved "unset" calls `env_remove(NAME)`. `jig` never wholesale-clears the inherited environment.
- If `cwd=` resolves to an effective working directory (§2.12), `jig` sets that as the child's working directory before `exec`. Otherwise the child inherits `jig`'s working directory. `jig` itself does not change its working directory; the user's shell is unaffected. A failure to change directory (path missing, permission denied, …) aborts the spawn with exit code 125 (§3.5) and a diagnostic naming the offending path.
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
5. Build the resolved argument list per §2.8:
   - Walk the command's children in source order. For each child:
     - If it is a default argument, append it to the candidate list.
     - If it is the selected profile, walk its children in source order, appending each to the candidate list.
     - If it is some other profile, skip.
   - Apply per-key resolution (§2.8 step 2): for each resolved CLI key, suppress per the `#false` rules (profile-side `#false` clears all default occurrences; default-side `#false` drops just itself), partition surviving entries into `+`-marked and unmarked, emit each marked entry at its source position, and resolve unmarked entries in single mode (≤ 1 on each side → v1 first-occurrence positioning with profile-value precedence) or repeat mode (otherwise → emit each unmarked at its source position).
6. For each remaining flag, format per the flag prefix rules (§2.5). Boolean `#true` flags emit only the flag key (no accompanying value). Positionals emit their literal value. Independently, resolve the effective working directory per §2.12.3 (walk the `extends` chain leaf-to-root for a profile-side `cwd=`, then fall back to the command's `cwd=`; resolve a relative value against the directory containing the loaded config file). The working-directory channel is orthogonal to the argv channel and may be computed in either order.
7. Append pass-through args at the end (§3.3).
8. If `--dry-run`: print shell-quoted command line; exit 0.
9. Otherwise: execute the resolved command from the effective working directory (if any). Exit with the child's exit status.

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

### 5.6 Repeated flags

Tools that accept the same flag more than once (`gcc -I /a -I /b`, `curl --header A --header B`, count flags like `-v -v -v`) are expressed by writing the flag multiple times. The merge algorithm picks repeat mode automatically when either side has two or more unmarked occurrences (§2.8 step 2.4). The `+` marker (§2.5 rule 0) is the explicit knob for the case the multiplicity rule cannot disambiguate: a single default and a single profile entry that should *add* rather than *replace*.

```kdl
gcc {
    I "/usr/include"
    I "/opt/include"

    project-a {
        I "/proj/a/include"        // repeat mode, appends
    }

    proj-extras {
        +I "/extra"                // explicit append against any defaults
    }

    bare {
        I #false                    // markerless "clear all defaults"
    }

    custom {
        I #false
        I "/mine"                   // clear, then add
    }
}

verbose-tool {
    v #true
    v #true
    v #true
}
```

| Command                          | Resolved |
|----------------------------------|----------|
| `jig gcc`                        | `gcc -I /usr/include -I /opt/include` |
| `jig gcc project-a`              | `gcc -I /usr/include -I /opt/include -I /proj/a/include` |
| `jig gcc proj-extras`            | `gcc -I /usr/include -I /opt/include -I /extra` |
| `jig gcc bare`                   | `gcc` |
| `jig gcc custom`                 | `gcc -I /mine` |
| `jig verbose-tool`               | `verbose-tool -v -v -v` |

### 5.7 Environment variables

`(env)NAME` declarations contribute to the spawned child's environment rather than its argv. Defaults apply unconditionally; profiles override or unset them per §2.11.

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

| Command                              | Resolved (`--dry-run`) |
|--------------------------------------|------------------------|
| `jig serve`                          | `env OLLAMA_HOST=0.0.0.0 CUDA_VISIBLE_DEVICES='0,1' llama-server --host 0.0.0.0` |
| `jig serve qwen-coder`               | `env OLLAMA_HOST=0.0.0.0 CUDA_VISIBLE_DEVICES=0 llama-server --host 0.0.0.0 -m /models/qwen-coder.gguf` |
| `jig serve sandbox`                  | `env -u OLLAMA_HOST CUDA_VISIBLE_DEVICES='0,1' llama-server --host 0.0.0.0 -m /models/sandbox.gguf` |

The `env(1)` prefix is the dry-run rendering only; actual execution applies the same outcomes via `Command::env` / `Command::env_remove` directly on the child (§3.6).

## 6. Out of Scope for v1

The following are deliberately deferred. None of them are precluded by the v1 design.

- Global / user-level config (`~/.config/jig/`).
- Environment variable interpolation in values.
- Templating, computed values, or includes.
- Multi-parent / diamond inheritance between profiles (single-parent inheritance via `extends=` is supported, §2.8.5).
- Multiple aliases per command.
- Multiple occurrences of the same profile within a command.
- Subcommand chains beyond what fits in a quoted command name.
- `--print` variants (e.g. argv-array form vs shell-quoted form).
- Validation of resolved commands beyond `Command::spawn` failures (e.g. proactive existence/executability checks before launching).

## 7. Resolved Design Decisions

These were open during spec drafting and have been resolved:

### 7.1 `--list` output format

Human-readable text only for v1. No JSON or other machine-readable form. May be added later if a real need emerges.

The output should be readable enough to grep and eyeball, but is not promised to be stable for scripting. Suggested format (non-normative, for implementation guidance):

```
config:   jig.kdl

llama-server  (alias: serve)
  cwd:      /home/me/llama-stack
  env:      -u OLD_VAR OLLAMA_HOST=0.0.0.0
  defaults: --host 0.0.0.0 --port 8090 -c 32768 --flash-attn
  profiles:
    qwen-coder
      args: -m /models/qwen-coder.gguf -ngl 999
      env:  CUDA_VISIBLE_DEVICES=0
    llama3
      args: -m /models/llama3.gguf --port 8091
    qwen-coder-large  (extends qwen-coder)
      cwd:  src
      args: -m /models/qwen-coder-large.gguf

rsync  (alias: sync)
  defaults: --archive --verbose
  profiles:
    backup
      args: /source/ user@host:/dest/
```

Output begins with a `config:` header line naming the loaded config file, followed by one blank line and then the per-command blocks. The path is rendered relative to the current working directory when possible (with `..` segments if the config sits in an ancestor), and falls back to the absolute path otherwise — matching the `config:` line emitted by `--explain` (§7.3).

For each command, the header line names the command and (if any) its alias. Then, in order, each command may emit:

- A `cwd:` line if the command has a `cwd=` property (§2.12). The value is shown as written in the config (an absolute path is shown absolute; a relative path is shown unchanged), not the resolved absolute path.
- An `env:` line if the command has default env-var contributions (§2.10). It mirrors the `env(1)` form used by `--dry-run`: `-u NAME` for unsets followed by `NAME=value` for sets.
- A `defaults:` line if the command has default arguments (§2.7), in source order, using the same shell-quoted token form as `--dry-run` (§7.2).
- A `profiles:` block listing each profile in source order. Each profile is shown as its own sub-block:
  - A header naming the profile and (if it inherits, §2.8.5) `(extends <parent>)`.
  - Optional `cwd:`, `env:`, and `args:` lines on the same channels as the command-level lines and using the same value formats. Each sub-line is omitted if the profile contributes nothing on that channel; a profile with an empty body is shown as just its header.

The per-profile sub-block is a **static** view: each profile's lines show what that profile alone contributes, not the resolved merge of defaults plus the profile (§2.8 / §2.11). Inheritance is named in the header (`extends`) but parent-profile contributions are not flattened in. To see what an invocation actually executes, use `--dry-run`.

Implementations may colorize the output when stdout is a terminal — typically command names, profile names, section labels, and parenthesized annotations — and must fall back to plain text when stdout is not a terminal or when the `NO_COLOR` environment variable is set.

### 7.2 `--dry-run` output format

Output the resolved command as a **single line, properly shell-quoted**, such that the line can be copy-pasted into a POSIX shell and executed with the exact same effect as omitting `--dry-run` would have produced.

This means:
- Arguments containing spaces, glob characters, quotes, or other shell-significant characters must be quoted (typically with single quotes, with embedded single quotes escaped using the standard `'\''` idiom).
- Arguments containing no shell-significant characters may be emitted unquoted for readability.
- The output goes to stdout. No trailing prompt, no leading `$`, no log decoration.

When env-var contributions (§2.10 / §2.11) are present, the line begins with an `env(1)` invocation that applies them, followed by the resolved command. The form is:

```
env [-u UNSET]... [NAME=value]... <program> <args>...
```

Unsets come first (one `-u NAME` per name); sets follow as `NAME=value` pairs; both names and values are shell-quoted via the same rules as above. With no env contributions, the prefix is omitted entirely and the output is byte-identical to a non-env config.

When an effective working directory (§2.12) is resolved, the whole line is wrapped in a `(cd <dir> && ... )` subshell so chdir is applied without altering the caller's shell. The cwd subshell sits outside the optional `env(1)` prefix, and the path is the resolved absolute path (the config's `cwd=` value applied against the config-file directory if it was relative). The composed form is:

```
[(cd <resolved-cwd> && ]\
  [env [-u UNSET]... [NAME=value]... ]\
  <program> <args>...\
[)]
```

The `(` / `)` and the `env` prefix appear together iff the corresponding feature is in use; either may be absent independently. With neither feature in use the output is byte-identical to a plain command line.

Example without env vars or cwd:
```
$ jig --dry-run serve qwen-coder
llama-server --host 0.0.0.0 --port 8090 -c 32768 --flash-attn -m /models/qwen-coder.gguf -ngl 999 -ts 0.5,0.5
```

Example with env vars (set + unset):
```
$ jig --dry-run serve qwen-coder
env -u OLD_VAR OLLAMA_HOST=0.0.0.0 CUDA_VISIBLE_DEVICES=0 llama-server --host 0.0.0.0 -m /models/qwen-coder.gguf
```

Example with a resolved cwd:
```
$ jig --dry-run serve qwen-coder
(cd /home/me/llama-stack && llama-server --host 0.0.0.0 -m /models/qwen-coder.gguf)
```

Example with both cwd and env vars:
```
$ jig --dry-run serve qwen-coder
(cd /home/me/llama-stack && env OLLAMA_HOST=0.0.0.0 llama-server --host 0.0.0.0 -m /models/qwen-coder.gguf)
```

Argv-style (one argument per line) is **not** offered in v1. If users need to inspect quoting, they can pipe the dry-run output to a shell parser, or we can revisit later.

### 7.3 Argv-resolution explanation (`--explain`)

`--explain` / `-x` traces how the resolved command line was assembled and exits without executing. For each emitted argv segment it names the contributing tier (defaults, an inheritance-chain ancestor, or the selected profile), the source `file:line`, and any merge decision (single-mode override, repeat mode, marker, `#null` ghost, `#false` suppression, middle-tier loss) that isn't already implied by the resolved line. Pass-through tokens (§3.2) supplied on the CLI invocation appear as a single trailing argv segment attributed to the command line, with no `file:line` since the tokens are not in the config. Env-var winners and shadowed contributions appear in a dedicated `env:` section; the effective `cwd=` appears in `cwd:`; keys whose every candidate was suppressed appear in `suppressed:` alongside the `#false` that cleared them.

Mutually exclusive with `--dry-run`, `--list`, and `--completions`. Where `--dry-run` shows *what* would be executed, `--explain` shows *why*.

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

### 7.5 Repeated flag keys

Repeating the same flag key within a scope is **allowed**, not a parse error. The merge algorithm in §2.8 picks the resolution mode per key:

- Single mode (≤ 1 unmarked occurrence on each side) preserves v1's first-occurrence positioning + profile-value override exactly. Every config that worked before this addition continues to resolve byte-identically.
- Repeat mode (any side with ≥ 2 unmarked occurrences) emits every unmarked occurrence at its source position, supporting `gcc -I /a -I /b`, count flags `-v -v -v`, and similar idioms without any new syntax.
- Profile-side `#false` clears all default occurrences of the key (asymmetric vs default-side `#false`, which only suppresses its own occurrence). This gives a markerless "replace defaults" idiom: a profile can write `K #false` followed by `K newvalue` to wipe a default list and substitute its own.
- The `+` marker (§2.5 rule 0) is the explicit knob for the only case the multiplicity rule cannot disambiguate: a single unmarked default plus a single unmarked profile entry that should *add* rather than *replace*. A `+`-prefixed flag always emits at its own source position and never collapses with unmarked occurrences.

The marker is rare by design. Adding a second occurrence anywhere is enough to put the key into repeat mode, where the marker is unnecessary.
