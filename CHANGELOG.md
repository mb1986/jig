# Changelog

All notable changes to `jig` are recorded in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

### Migration note

- If you used `a #false` in defaults expecting it to reserve a
  position for a profile to fill in (the v0.2.0 behavior), use
  `a #null` in 0.6.1+. The `#false` behavior is unchanged from
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

[Unreleased]: https://github.com/mb1986/jig/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/mb1986/jig/releases/tag/v0.6.0
[0.5.0]: https://github.com/mb1986/jig/releases/tag/v0.5.0
[0.4.0]: https://github.com/mb1986/jig/releases/tag/v0.4.0
[0.3.1]: https://github.com/mb1986/jig/releases/tag/v0.3.1
[0.3.0]: https://github.com/mb1986/jig/releases/tag/v0.3.0
[0.2.0]: https://github.com/mb1986/jig/releases/tag/v0.2.0
[0.1.0]: https://github.com/mb1986/jig/releases/tag/v0.1.0
