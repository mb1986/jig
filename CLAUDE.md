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
