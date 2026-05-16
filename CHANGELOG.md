# Changelog

All notable changes to `jig` are recorded in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.10.0] — 2026-05-16

### Added

- `--cat` flag. Writes the loaded config file's body to stdout and
  a `cat <path>` header to stderr (bold on a terminal), so
  `jig --cat | grep …` sees only the file content. The header
  reports a shell-quoted, cwd-relative path. Works even when the
  file fails to parse, so a broken config can still be inspected.
  Mutually exclusive with `--list`, `--dry-run`, `--explain`, and
  `--completions`; tab completion for zsh, bash, and fish offers it.
- `--list` output now starts with a `config:` header line naming the
  loaded config file, mirroring the `config:` line at the top of
  `--explain` output. The path is rendered relative to the current
  working directory when possible (with `..` segments if the config
  sits in an ancestor), and falls back to the absolute path otherwise.

### Changed

- Shell completion scripts (zsh, bash, fish) now honor jig's
  mutual-exclusion graph. Once a flag is on the command line, the
  flags it conflicts with are dropped from `jig -<TAB>` candidates —
  typing `jig --list <TAB>` no longer offers `--cat` or `--explain`,
  `jig --cat <TAB>` drops `--list`, `--dry-run`, `--explain`, and
  `--completions`, and so on, with the relation applied symmetrically.
  When `--list`, `--cat`, or `--completions` is present, command and
  profile candidates are also suppressed (no command name is needed
  in those modes). `--explain` and `--dry-run` continue to offer
  command/profile candidates because they operate on a command.

### Fixed

- Shell completion scripts (zsh, bash, fish) now offer `-x` /
  `--explain` as candidates. The flag landed in 0.9.0 but was not
  added to the completion scripts at the time, so tab-completing
  `jig -<TAB>` never surfaced it.

## [0.9.1] — 2026-05-15

### Fixed

- `--explain` now surfaces CLI-supplied pass-through tokens as a
  trailing argv segment with a `from command line` attribution.
  Previously the tokens were silently dropped from `--explain` output
  even though they still flowed through to the executed command and
  to `--dry-run`.

## [0.9.0] — 2026-05-15

### Added

- `--explain` / `-x` traces how the resolved command was assembled
  and exits without executing. The header names the program, the
  loaded config (relative to cwd), the selected profile, and the
  inheritance chain when one applies. The resolved argv is rendered
  inline with dim `[N]` markers, each pointing to a numbered footnote
  that names the contributing tier(s), source `file:line`, and any
  merge decision worth surfacing — single-mode override, repeat mode,
  marker (`+`), `#null` ghost (annotated `(#null ghost — no value)`),
  middle-tier loss in a 3+ tier chain, and `#false` suppression. A
  dedicated `env:` section lists env-var winners and shadowed
  lower-tier contributions; `cwd:` names the effective working
  directory; `suppressed:` lists keys whose every candidate was
  cleared, alongside the `#false` that cleared them. Mutually
  exclusive with `--dry-run`, `--list`, and `--completions`.

## [0.8.3] — 2026-05-15

### Fixed

- Shell completion scripts (zsh, bash, fish) now offer `-q` /
  `--quiet` and `--completions` as candidates. Both were already
  recognised when parsing argv context, so tab completion behaved
  correctly *around* them, but neither flag showed up when
  tab-completing `jig -<TAB>`. `--completions` also offers
  `zsh` / `bash` / `fish` as value candidates.
- Value completion for `--config` and `--completions` no longer
  fires after the first positional. Once a command name has been
  entered, those tokens are pass-through to the child command and
  the next word belongs to the child — not jig — so the shell
  must not offer file candidates (for `--config`) or shell names
  (for `--completions`) as if jig still owned the flag.

## [0.8.2] — 2026-05-15

### Changed

- `--completions <SHELL>` is now shown in `--help`. Previously
  hidden on the assumption that users wouldn't type it directly,
  but it's the documented install path for tab completion on a
  crates.io-distributed CLI and deserves visibility. The internal
  `--list-commands` / `--list-profiles` flags stay hidden.

### Fixed

- A node with a child block inside a profile body is now rejected
  as a parse error (`profiles cannot be nested`) instead of being
  silently dropped. Spec §2.3 already said profiles do not nest in
  v1, but the parser would walk such a node through the argument
  path and discard its children, so misplaced nesting produced no
  diagnostic and looked like the inner flags had been accepted.
- Fish completion now matches the zsh/bash behavior of treating
  every token after the first positional — including ones that
  start with `-` — as command/profile/pass-through context. Before
  the fix, typing `jig serve -x <TAB>` still routed to profile
  completion because the helper counted `-x` as a jig flag. Now
  it routes to file completion via the `-F` rule, matching the
  argv split in `src/cli.rs`.
- Pre-exec preview rendering is now best-effort: when shell-quoting
  fails (a non-UTF-8 pass-through arg, a non-UTF-8 cwd, or a NUL
  byte that `shlex` cannot quote), `jig` writes a one-line
  `preview unavailable: …` notice to stderr and continues to the
  spawn instead of erroring out. Previously the wrapper exited 125
  even though `std::process::Command` itself would have happily
  accepted the `OsString`/`Path`. `--dry-run` stays strict — the
  rendered line IS the output there, so a render failure is still
  a real failure.

## [0.8.1] — 2026-05-15

### Fixed

- `rust-version` in `Cargo.toml` is now `1.88`, matching the real
  minimum compiler the source needs (stable let-chain expressions in
  `src/resolve.rs`). Previous releases declared `1.85` (the edition
  2024 floor), which would have surfaced a hard compile error on a
  1.85 toolchain instead of cargo's "package requires a newer rustc"
  message. Installs on 1.88+ are unaffected.

## [0.8.0] — 2026-05-15

### Added

- Working directory for the spawned child (SPEC §2.12). A KDL
  property `cwd="<path>"` on a command or profile node pins the
  directory the child runs from. Absolute paths are used as written;
  relative paths resolve against the directory containing the loaded
  config file, so `cwd="."` means "run from the config-file
  directory" and project-relative paths work no matter how deep the
  user's current directory is. The property layers like flags: a
  profile-side `cwd=` overrides a command-side one, and within an
  `extends` chain the leaf wins. `--list` shows a `cwd:` line per
  command when set, and `--dry-run` wraps the resolved invocation in
  a `(cd <resolved-cwd> && ...)` subshell so the line is still
  copy-pasteable. A failure to enter the target directory aborts the
  spawn with exit code 125 and a diagnostic naming the path. No
  suppression form is provided in v1 (a profile cannot unset its
  command's `cwd=` to fall back to the user's CWD).
- Pre-exec preview line (SPEC §3.4.1). Before spawning the resolved
  command, `jig` writes a single shell-quoted line to stderr showing
  exactly what it is about to run (same format as `--dry-run`,
  including the leading `env(1)` prefix when env vars are set). The
  line is rendered in bold when stderr is a terminal and as plain
  text when stderr is redirected, so log files and grep output stay
  readable. The preview is suppressed under `--dry-run` (which
  already prints the resolved line) and on every non-exec path
  (`--list`, `--completions`, etc.).
- `-q` / `--quiet` flag to suppress the pre-exec preview line.

### Changed

- `--list` output (SPEC §7.1). Each profile now renders as its own
  indented sub-block with optional `cwd:`, `env:`, and `args:` lines
  built from the profile's own contributions, alongside the existing
  command-level `cwd:`, `env:`, and `defaults:` lines (the
  command-level label `default-args:` is now `defaults:`). Section
  labels within a block are padded to a common width so values line
  up. When stdout is a terminal and `NO_COLOR` is unset, command
  names, profile names, section labels, values, and the
  `(alias: ...)` / `(extends ...)` annotations each get their own
  ANSI style (section labels dim, values bright white); piped or
  `NO_COLOR`-set output stays plain text. The per-profile view is
  static — it shows each profile's raw contributions, not the
  resolved merge with defaults; use `--dry-run` to see what a given
  invocation would actually execute.
- Configuration discovery now walks upward from the current working
  directory through its ancestors instead of looking only at the CWD
  (SPEC §2.1). Within each directory `jig.kdl` is preferred over
  `.jig.kdl`; the first directory containing either file ends the
  search and only the nearest configuration is loaded. The walk is
  bounded by `$HOME`: when `$HOME` appears in the ancestor chain it
  is the last directory checked, otherwise the walk continues to the
  filesystem root. `--config <PATH>` continues to skip discovery
  entirely, and `jig` does not change its working directory when the
  configuration is found in an ancestor. The "config file not found"
  diagnostic now reports the starting directory and the last
  directory actually checked, replacing the previous "in directory:
  <cwd>" line.

## [0.7.1] — 2026-05-14

### Fixed

- Shell completion now expands a leading `~` in the value passed to
  `--config` before forwarding it to `jig --list-commands` /
  `--list-profiles`. Previously, `jig --config ~/cfg.kdl <TAB>`
  silently returned no candidates because the completion script
  fed the literal `~/cfg.kdl` token back to `jig`, which could not
  open the file. Affects the zsh, bash, and fish scripts. The fix
  handles `~` and `~/` only; `~user/` is intentionally not expanded
  (uniform across all three shells — zsh's built-in `${~var}` would
  cover `~user/` but writes "no such user" to stderr on unknown
  accounts, which would be visible mid-tab).

## [0.7.0] — 2026-05-13

### Added

- The `#null` placeholder (SPEC §2.4.3). A flag whose value is the
  KDL keyword `#null` is a position-only marker: it declares the
  flag at this source position but contributes no value, suppresses
  nothing, and is never emitted on argv. Its source position feeds
  the first-occurrence rule, so a later survivor of the same key
  (typically from a profile or an inherited tier) emits at the
  placeholder's slot. Idiomatic for declaring command-level
  documentation — listing every flag a command supports at canonical
  positions and letting profiles fill in actual values — where you
  want the placeholder to be visible in the config but absent from
  the resolved command when no value is supplied. The `+` append
  marker on a `#null` is rejected at parse time.

### Changed

- A flag whose value is the KDL keyword `#null` is now interpreted
  per SPEC §2.4.3 (see the Added entry). In v0.6.x and earlier the
  same syntax fell through the generic value path and emitted as
  the literal four-character string `#null` on argv. Configs that
  relied on that behaviour (rare — `#null` would have been an odd
  value to pass) should switch to the quoted form `"#null"` to
  preserve the literal-string semantics, per the §2.4.1 convention.

### Migration note

- If you used `a #false` in defaults expecting it to reserve a
  position for a profile to fill in (the v0.2.0 behavior), use
  `a #null` in 0.7.0+. The `#false` behavior is unchanged from
  v0.3.0+: it remains a "remove this flag" marker that drops the
  occurrence including its position. `#null` is the dedicated
  placeholder tool.

## [0.6.0] — 2026-05-13

### Added

- Profile-to-profile inheritance via the `extends="<parent>"` property
  on a profile node (SPEC §2.8.5). A child profile inherits its
  parent's body and may override individual flags or env vars; the
  parent may itself inherit from a grandparent, and so on. Selecting
  the leaf activates every profile in the chain — each one's body
  emits at its own source position — and the merge algorithm
  generalises to an N-tier cascade where the highest-tier value wins
  at the earliest source position. The `extends` graph is restricted
  to a single parent per profile, must be acyclic, and references
  only profiles within the same command; unknown parents and cycles
  are rejected at validation time with diagnostics that label every
  participating site. Inheriting profiles render in `--list` output
  as `<name> (extends <parent>)`.

### Changed

- The single-mode position rule in the per-key merge (SPEC §2.8.1)
  is now strictly "earliest source index among unmarked survivors".
  This is the spec-correct generalisation that two-tier-only code
  approximated as "default's slot if present, else profile's"; the
  rules coincide whenever defaults precede the selected profile in
  source order (the typical layout), so every pre-existing test
  resolves byte-identical. The corner where they diverge is a
  profile slot that textually precedes its overriding default with
  other source content in between — that configuration now emits the
  merged occurrence at the profile's slot, matching the §2.8.1
  wording.

## [0.5.0] — 2026-05-10

### Changed

- A literal `--` immediately after the command-or-alias is now consumed
  as the "no profile selected" marker, and everything from the next
  token onward is pass-through (SPEC §3.1). This unblocks the
  previously-impossible `jig <cmd> <bare-positional>` case: a positional
  that is not a profile name (and not hyphen-prefixed) used to be
  misread as a profile, producing an "unknown profile" error. Write
  `jig <cmd> -- <positional>` instead and the bare token reaches the
  child. A `--` written *after* a real profile, or after a previously-
  consumed `--`, still sits in the pass-through region and is preserved
  verbatim per §3.2. To pass a literal `--` as the first pass-through
  token when no profile is selected, write `--` twice: the first is the
  no-profile marker (consumed), the second is preserved.

## [0.4.0] — 2026-05-10

### Added

- Environment-variable contributions on commands and profiles (SPEC
  §2.10 / §2.11). A KDL node bearing the `(env)` type annotation
  declares an env var rather than a CLI argument: `(env)NAME "value"`
  sets the variable on the spawned child via `Command::env`, and
  `(env)NAME #false` removes it via `Command::env_remove`. Profile
  declarations override defaults under the same precedence as flags;
  duplicates within one scope are a parse error and unknown
  annotations (`(cwd)`, `(file)`, …) are rejected so the design
  space stays open for future use.
- `--dry-run` prefixes the resolved command with an `env(1)`
  invocation when env contributions are present (`env -u UNSET
  NAME=value … program …`), so the line remains copy-paste-correct
  in any POSIX shell. Without env contributions the output is
  byte-identical to before.
- `--list` emits an `env:` line for commands that declare env
  defaults, mirroring the `env(1)`-style form used by `--dry-run`.

## [0.3.1] — 2026-05-10

### Fixed

- Shell completion now respects jig's argv split. Tabbing at a
  hyphen-prefixed token after the command name (`jig serve -<TAB>`,
  `jig serve qwen-coder --<TAB>`) no longer re-offers jig's own
  flags as candidates — the cursor is in pass-through territory, so
  the zsh script falls through to `_files` and bash falls through to
  `compgen -f`. The positional walk used to dispatch dynamic
  candidates is also fixed: hyphen-prefixed pass-through tokens
  (`jig serve -x <TAB>`) are no longer miscounted as flags, so the
  scripts correctly land in the pass-through branch instead of
  re-offering profile candidates for the command. Affects `jig.zsh`
  and `jig.bash`; `jig.fish` already counted positionals correctly.

## [0.3.0] — 2026-05-07

### Added

- Repeated flag keys are now allowed within a scope. The merge algorithm
  in SPEC §2.8 picks single-mode (≤ 1 unmarked occurrence on each side →
  v1 first-occurrence positioning + profile-value override) or repeat
  mode (otherwise → emit every unmarked occurrence at its source
  position) per key. This unblocks tools that legitimately accept the
  same flag more than once: `gcc -I /a -I /b`, `curl --header A --header B`,
  count flags like `-v -v -v`. v1 configs resolve byte-identically.
- Profile-side `#false` for a key now clears every default occurrence of
  that key (not just one). Combined with subsequent `K newvalue` lines
  this is the markerless "replace defaults" idiom: `bare { I #false }`
  wipes a multi-default `-I` list; `custom { I #false; I "/mine" }`
  replaces it with one new entry.
- `+` flag-key prefix as the explicit append marker (SPEC §2.5 rule 0).
  Writing `+I "/proj"` in a profile forces that occurrence to emit at
  its own position rather than collapsing with an unmarked default of
  the same key. This handles the only case the multiplicity rule cannot
  disambiguate: a single default plus a single profile entry that should
  *add* rather than *replace*.

### Removed

- The `DuplicateFlagKey` diagnostic (and the §2.9 constraint behind it)
  is gone. Repeating a key is no longer a parse error.

## [0.2.0] — 2026-05-06

### Added

- Dynamic shell completion. `jig --completions zsh|bash|fish` now emits a
  hand-rolled script that completes command names, aliases, and profile
  names from the local `jig.kdl` at completion time. The script forwards
  an explicit `--config <PATH>` from the user's command line so candidates
  always reflect the chosen config. Backed by two new hidden flags,
  `--list-commands` and `--list-profiles <COMMAND>`, which print one
  candidate per line and exit `0` silently on any failure so completion
  never breaks mid-tab.

### Changed

- A command name may now appear more than once across the config, as long
  as every occurrence declares a distinct alias. Duplicated names are no
  longer valid lookup keys — invocations must use one of the aliases. A
  command name that appears exactly once continues to be a valid lookup
  key (no behavior change for existing configs). A new `AmbiguousCommand`
  diagnostic is raised when the bare form of a duplicated name is invoked.
- `--completions` no longer accepts `elvish` or `powershell`. The supported
  shells are now `zsh`, `bash`, and `fish`. The `clap_complete` dependency
  has been dropped in favor of hand-rolled scripts that support dynamic
  completion.

## [0.1.0] — 2026-05-04

Initial release. v1, Unix only (Linux and macOS).

### Added

- KDL-based configuration (`./jig.kdl` or `./.jig.kdl`, or `--config <PATH>`)
  with named commands, optional aliases, command-level default arguments,
  and named profiles.
- Argument model: KDL nodes-with-value become flags, nodes-without-value
  become positionals; `#true` / `#false` toggle flag presence; explicit
  dash-prefixed keys pass through verbatim; single-character inferred keys
  get `-`, longer keys get `--`.
- Source-order resolution with first-occurrence positioning for flag
  overrides and `#false` as a universal suppression marker.
- CLI: `--dry-run` (shell-quoted preview), `--list`, `--config`,
  `--completions <SHELL>`, `--help`, `--version`. Pass-through arguments
  trail the resolved command line and preserve a literal `--`.
- Wrapper-tool exit codes (`0` propagated, `125` for `jig`-internal
  failure, `126` for not-executable, `127` for not-found).
- Diagnostic-quality error rendering via `miette`, with source spans for
  parse and constraint errors and did-you-mean hints for unknown command
  / alias / profile.
- Static shell completion for bash, zsh, fish, elvish, and powershell via
  `clap_complete`.

[Unreleased]: https://github.com/mb1986/jig/compare/v0.10.0...HEAD
[0.10.0]: https://github.com/mb1986/jig/releases/tag/v0.10.0
[0.9.1]: https://github.com/mb1986/jig/releases/tag/v0.9.1
[0.9.0]: https://github.com/mb1986/jig/releases/tag/v0.9.0
[0.8.3]: https://github.com/mb1986/jig/releases/tag/v0.8.3
[0.8.2]: https://github.com/mb1986/jig/releases/tag/v0.8.2
[0.8.1]: https://github.com/mb1986/jig/releases/tag/v0.8.1
[0.8.0]: https://github.com/mb1986/jig/releases/tag/v0.8.0
[0.7.1]: https://github.com/mb1986/jig/releases/tag/v0.7.1
[0.7.0]: https://github.com/mb1986/jig/releases/tag/v0.7.0
[0.6.0]: https://github.com/mb1986/jig/releases/tag/v0.6.0
[0.5.0]: https://github.com/mb1986/jig/releases/tag/v0.5.0
[0.4.0]: https://github.com/mb1986/jig/releases/tag/v0.4.0
[0.3.1]: https://github.com/mb1986/jig/releases/tag/v0.3.1
[0.3.0]: https://github.com/mb1986/jig/releases/tag/v0.3.0
[0.2.0]: https://github.com/mb1986/jig/releases/tag/v0.2.0
[0.1.0]: https://github.com/mb1986/jig/releases/tag/v0.1.0
