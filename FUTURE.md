# Future Ideas (post-v1)

Captured for later consideration. **Not part of v1 scope.** This document
exists so good ideas surfaced during design discussions are not lost,
without polluting the v1 spec.

Each entry below is a sketch, not a commitment. Some may be implemented
in v2; others may be discarded after more thought.

## Pre-exec validation

Type annotations that perform a check before launching:

- `(file)m "/models/foo.gguf"` — fail with a clear error if the file
  doesn't exist.
- `(executable)cmd "/usr/local/bin/foo"` — fail if not present or not
  executable.
- `(dir)data-dir "/var/data"` — fail if not a directory.

Failures should produce typed `jig` errors with helpful messages
(exit code 125), not pass through to the target command.

## Profile selection from environment

Allow `JIG_PROFILE=qwen-coder jig serve` (or similar) to select a
profile from an env var, useful in CI / scripted contexts where the
profile name is determined dynamically.

Should layer cleanly with explicit profile arguments on the CLI:
explicit profile wins, env var is the fallback.

## Templating / interpolation

`m "${HOME}/models/foo.gguf"` — env var expansion in values. Tempting
but tricky:

- Conflicts with literal `${...}` in values that some tools accept.
- Quoting rules across shells differ.
- Could expand to template engines (mustache-style) if not careful.

Probably worth a separate, narrowly scoped proposal before
implementation. Start with env var expansion only, with explicit opt-in
syntax (e.g. `(template)m "..."` or escape forms like `${{ HOME }}`
that don't collide with shell syntax).

## JSON output for `--explain` / `--list`

`--explain` ships in 0.9.0 with a human-readable layout (header +
annotated argv with numbered footnotes per emitted segment). A
machine-readable form for use by scripts and other tooling would
add a `--format=json` (or sibling) flag; the trace data model in
`src/resolve.rs` (`ResolutionTrace` / `SegmentTrace` / `Contributor`
/ `EnvTrace` / `CwdTrace`) is already shaped for serialisation —
adding `serde::Serialize` derives and a JSON renderer would be a
small follow-up. Wait for a real consumer before implementing.

The same applies to `--list`: the plain-text form is sufficient for
the dynamic completion shipped today (which uses `--list-commands` /
`--list-profiles`, not `--list`); add JSON only when a real consumer
asks for it.

## Library crate

Refactor the binary into a library (`jig-run`) plus a thin binary
(`jig`). Allows other Rust tools to embed `jig`'s resolution logic,
reuse the config types, etc. Non-breaking for v1 users.

## Multi-parent profile inheritance

Single-parent inheritance via `extends="<parent>"` is supported
(see `SPEC.md` §2.8.5 and `CHANGELOG.md`). The natural follow-up is
letting a profile list multiple parents — useful when two orthogonal
"mixins" want to combine (e.g. `gpu` + `large-context`). Open questions:

- How is the parent list expressed in KDL? A space-separated string
  is ambiguous if parent names can contain hyphens; a child node
  feels heavier than the single-parent property; KDL doesn't have
  native list-of-strings syntax for properties.
- Merge order across multiple parents (left-to-right wins? MRO?
  user-controlled?). Each option has subtle interactions with the
  N-tier cascade in §2.8.5.
- Diamond resolution: if A extends [B, C] and both B and C extend D,
  does D's body activate once or twice?

Wait for a demand signal before implementing; single-parent covers
the immediate use case.

## Multiple aliases per command

Currently a command has at most one alias. Multiple aliases would be
trivially supported by changing the alias representation, but no
compelling use case has surfaced.

## Repeating the same profile within a command

KDL allows the same node name to appear multiple times. We currently
forbid this for profile names (per `SPEC.md` §2.9), but the underlying
parser preserves duplicates. A future feature could allow the same
profile to be referenced at multiple positions within a command, so
that selecting it injects its arguments at each of those positions.

Example use case:

```kdl
some-tool {
    profile-x { timeout 30 }
    flag-a "x"
    profile-x { verbose #true }
}
```

When `jig some-tool profile-x` is invoked, the walk algorithm in §2.8
already produces the correct candidate list — `[timeout=30, flag-a=x,
verbose=true]` — without any changes. The work to add this feature is
almost entirely in validation.

### What needs to change

1. **Drop the §2.9 uniqueness constraint for profiles.** The merge
   algorithm already handles the multi-occurrence case correctly
   because it is defined as a single source-order walk.

2. **Decide override semantics across blocks.** When the same flag key
   appears in multiple blocks of the same profile (e.g.
   `profile-x { timeout 30 }` and `profile-x { timeout 60 }`),
   first-occurrence positioning collapses them into one entry. Which
   value wins?

   - **Last wins** (recommended): mirrors shell variable assignment and
     standard CLI parser behavior; the latest value is what sticks.
   - **First wins**: earliest profile contribution sticks.
   - **Error**: same key in multiple blocks of the same logical profile
     is a constraint violation.

   "Last wins" is the most intuitive default and the easiest to reason
   about.

3. **Reinterpret §2.9 "each flag key appears at most once per scope".**
   With multiple blocks contributing to one logical profile, "scope"
   becomes ambiguous. Two choices:

   - **Per-block** (recommended): each individual `profile-x { ... }`
     block must have unique keys; across blocks, duplicates are allowed
     and resolved by rule 2.
   - **Per-logical-profile**: all blocks of a given profile name in a
     given command, taken together, must have unique keys.

   Per-block is simpler to validate (no cross-block bookkeeping at
   parse time) and naturally pairs with last-wins semantics.

### V1 type design that makes this trivial

The naive type design for v1 would be something like:

```rust
struct Command {
    name: String,
    alias: Option<String>,
    defaults: Vec<Argument>,
    profiles: HashMap<String, Profile>,
}
```

This separates defaults from profiles, and uses a hashmap that
structurally enforces profile-name uniqueness. Adding multi-occurrence
support later would require restructuring the type and rewriting the
walk.

A better v1 design — recommended in `IMPLEMENTATION.md` §7.3 — is:

```rust
struct Command {
    name: String,
    alias: Option<String>,
    children: Vec<CommandChild>,  // source-ordered
}

enum CommandChild {
    Default(Argument),
    Profile { name: String, args: Vec<Argument> },
}
```

This directly models "a command's body is a source-ordered list of
defaults and profile-blocks," which is what the spec already says.
Profile-name uniqueness is enforced by a separate validation pass over
the `children` vec, not by the type. To lift the constraint in v2, you
delete that validation pass — no other code changes needed.

The `Vec` is iterated to find profile matches by name. For v1, where
each name appears at most once, this is O(n) but n is tiny (typically
< 20 profiles per command). If profile-lookup performance ever matters,
a separate `HashMap<String, Vec<usize>>` index can be built alongside
the `Vec`, mapping profile names to indices in `children`.

### Open questions

- What if the multiple occurrences have *different bodies* such that a
  user could plausibly want "logically separate" profile definitions
  with the same name? (Treat as one merged profile, or as an error?
  Recommendation: merge — there is no way to disambiguate them on the
  command line anyway.)
- Should the `--list` output show "profile-x (3 occurrences)" or just
  "profile-x"? The latter is simpler; users who care can read the
  config.

The v1 constraint stays in place until a real demand signal emerges,
but the v1 type design will be chosen so that lifting it later is a
matter of removing one validation rule, not a refactor.

## Windows support

v1 targets Unix only (Linux, macOS). Windows is deferred. Adding it
later means revisiting at minimum:

- **Path handling.** The "treat as path if it contains a path
  separator" rule (SPEC §3.6) is currently `/`-only. Windows uses both
  `\` and `/` as separators, and the conventional rule for whether a
  bare name is a path is more involved (drive letters, UNC, `.exe`
  resolution).
- **Shell-quoting for `--dry-run`.** `shlex` produces POSIX-shell
  quoting. Pasting the dry-run output into `cmd.exe` or PowerShell
  would mis-quote anything containing spaces, quotes, or shell
  metacharacters. A Windows port would need a quoting variant per
  target shell (or output a normalized form and document the caveat).
- **Signal mapping.** SPEC §3.5's `128 + signum` convention for
  signal-killed children is Unix-only. Windows has no analogous
  signal model; the wrapper-tool exit code would have to be redefined
  (most likely just propagating `ExitStatus::code()` and dropping the
  signal-derived range).
- **Process group / ctrl-c semantics.** SPEC §3.6 leans on Unix
  process-group inheritance for SIGINT/SIGTERM forwarding. Windows
  console control events behave differently and would need explicit
  handling.

None of this is precluded by v1's design; it is just additional work
that would not pay off for the v1 audience.
