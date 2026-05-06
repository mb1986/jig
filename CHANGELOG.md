# Changelog

All notable changes to `jig` are recorded in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
