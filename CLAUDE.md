# Claude Code Working Notes for `jig`

## Read first

Before doing anything in this repo, read:

1. `SPEC.md` — the behavioral specification. This is the source of truth
   for *what* `jig` does. Do not deviate from it without asking first.
2. `IMPLEMENTATION.md` — the implementation guide. This is the source of
   truth for *how* `jig` is built (deps, structure, code quality, types,
   testing). Do not deviate from it without asking first.

`FUTURE.md` also exists in the repo and lists features deferred to
post-v1. You do not need to read it to implement v1. Consult it only if:
  - You're tempted to add a feature — check there first to see if it
    has been explicitly deferred.
  - A user request implies scope that doesn't fit v1 — suggest it goes
    into FUTURE.md rather than into v1.

If a request appears to conflict with `SPEC.md` or `IMPLEMENTATION.md`,
stop and ask rather than guessing.

## Working principles

- **Spec-Driven Development.** All behavior must trace back to `SPEC.md`.
  If something isn't in the spec, it's not in the code.
- **No dead code.** No `#[allow(dead_code)]` "for later." If it's not
  needed for v1, don't write it.
- **No speculative abstractions.** Build the simplest thing that meets
  the spec. Refactor when a real second use-case appears, not before.
- **No `unwrap()` / `expect()` in non-test code,** except for documented
  invariants using `expect("invariant: ...")`.
- **No `unsafe`.**
- **No `clone()` to evade the borrow checker.** Restructure instead.
- **Lints:** `clippy::pedantic` and `clippy::nursery` are warnings;
  `unsafe_code` and `missing_docs` are denied/warned per
  `IMPLEMENTATION.md` §6.1. Targeted `#[allow]` is acceptable but must
  carry a comment explaining why.
- **Test as you go.** Each module gets unit tests in the same commit
  that introduces it. Snapshot tests (`insta`) for `--dry-run` output
  and error-message rendering. Integration tests in `tests/` for
  end-to-end behavior.

## Workflow expectations

- Before writing code, briefly state the plan and which spec sections
  it implements. Wait for approval at non-trivial decision points.
- Quote the relevant sentence from `SPEC.md` when answering a behavioral
  question rather than relying on memory or intuition.
- After writing code, run `cargo fmt`, `cargo clippy -- -D warnings`,
  and `cargo test`. Fix any issues before declaring done. Paste the
  output if anything fails.
- Keep commits small and focused. One logical change per commit, with
  a clear message describing what and why.
- If you discover a spec ambiguity, gap, or apparent contradiction,
  raise it explicitly rather than choosing a behavior unilaterally.
- If you find yourself reaching for a workaround, stop and check
  whether the underlying assumption is correct.

## What I (the user) value

- Clean, readable Rust over clever Rust.
- Errors that help the user fix the problem (see `SPEC.md` §7.4).
- A small, sharp tool. Resist scope creep.
- Honest acknowledgment when something is uncertain, incomplete, or
  worth a second look — not false confidence.

## User-facing strings

- **No `SPEC.md` / `IMPLEMENTATION.md` / `§N.M` references in anything
  the running program prints** — `--help` text, `--list` output,
  `miette` error messages, hints. clap derives flag help from the doc
  comments above each `#[arg(...)]` field, so those doc comments are
  user-facing too: keep spec pointers out of them. SPEC pointers in
  module-level doc comments and tests are fine (they're for
  contributors).
- Diagnostics should hold up on their own: a one-line summary, a
  span/label when relevant, a `help:` line with concrete next steps.
- README links to `SPEC.md` whole, not to specific section numbers
  (numbers drift; the link should stay valid).

## Versioning and releasing

`jig` follows [SemVer](https://semver.org). The crate is published as
`jig-run` on crates.io (the bare `jig` name is taken); the binary is
`jig`. Release artefacts: a git tag `vX.Y.Z`, a crates.io upload, and
a GitHub release. Repo: `mb1986/jig`. Remote: `git@github.com:mb1986/jig.git`.

### Pre-flight checklist

Before tagging anything, in order:

1. Working tree clean. All work merged to `main`.
2. `cargo fmt --check` clean.
3. `cargo clippy --all-targets -- -D warnings` clean.
4. `cargo test` — all unit + integration tests pass.
5. `cargo publish --dry-run` — confirm it packages cleanly. Then
   `cargo package --list` and eyeball the file list: only runtime
   sources + `README.md` + `LICENSE` + `CHANGELOG.md` + `SPEC.md` +
   `IMPLEMENTATION.md` should ship. The `include` list in
   `Cargo.toml` is the source of truth — adjust it if a new
   user-relevant top-level file lands.
6. Bump `version` in `Cargo.toml` (and refresh `Cargo.lock` via a
   `cargo build`).
7. Update `CHANGELOG.md`: rename the `[Unreleased]` heading to
   `[X.Y.Z] — YYYY-MM-DD`, add a fresh empty `[Unreleased]`, and
   update the link references at the bottom.

### Commits, tag, push, publish

8. Group commits logically — one logical change per commit, never one
   "release prep" megacommit. Typical groupings: code/docs cleanup,
   `Cargo.toml` metadata, `CHANGELOG.md`, CI changes, new tests.
9. Annotated tag: `git tag -a vX.Y.Z -m "jig X.Y.Z — <one-line>"`.
   Always annotated, never lightweight.
10. Push: `git push origin main && git push origin vX.Y.Z`.
11. `cargo publish`. If `cargo login`'s interactive prompt fails to
    receive a paste (this happens when stdin isn't a real TTY), use
    `CARGO_REGISTRY_TOKEN=<token> cargo publish` instead. Publishing
    is one-way — `cargo yank` exists, but `cargo unpublish` does not
    after the first 72 hours.
12. GitHub release:
    ```sh
    gh release create vX.Y.Z --repo mb1986/jig \
      --title "jig X.Y.Z" --verify-tag \
      --notes "$(cat <<'EOF'
    <copy the [X.Y.Z] section of CHANGELOG.md here, plus a
    crates.io install hint and a link back to the changelog>
    EOF
    )"
    ```
    `--verify-tag` makes `gh` refuse to create the release if the tag
    doesn't already exist on the remote — guards against typos.

### Never

- Never publish from a dirty working tree.
- Never tag a commit that hasn't been pushed.
- Never use `cargo publish --allow-dirty` for a real release. (It's
  fine for local `--dry-run` experiments.)
- Never force-push `main` or rewrite a published tag.
