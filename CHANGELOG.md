# Changelog

All notable changes to `jig` are recorded in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/mb1986/jig/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/mb1986/jig/releases/tag/v0.2.0
[0.1.0]: https://github.com/mb1986/jig/releases/tag/v0.1.0
