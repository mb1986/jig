//! Resolve a CLI invocation against a parsed [`Config`] into the
//! candidate argument list per `SPEC.md` §2.8, §2.8.5, and §4.
//!
//! The algorithm:
//!
//! 1. Look up the named command by name, then by alias. Return
//!    [`Error::UnknownCommand`] (with did-you-mean) on miss.
//! 2. If a profile name was supplied, look it up within the matched
//!    command. Return [`Error::UnknownProfile`] (with did-you-mean
//!    and an "available" list) on miss.
//! 3. Build the inheritance chain for the selected leaf by walking
//!    `extends` pointers (§2.8.5). Validation guarantees the chain
//!    is acyclic and that every parent resolves to a sibling
//!    profile, so the walk is total. Tier 0 is defaults; the root
//!    ancestor is tier 1; the selected leaf is tier N.
//! 4. Walk the command's children in source order. Defaults push
//!    one candidate each at tier 0; every profile in the chain
//!    pushes its body's children at its own tier; other profiles
//!    are skipped (`SPEC.md` §2.7).
//! 5. Group flag candidates by resolved CLI form (per §2.5). For
//!    each key build an emission plan per the per-key cascade in
//!    `SPEC.md` §2.8 / §2.8.5:
//!    - Suppression: a `#false` at tier T > 0 drops every entry at
//!      tiers `< T` for that key. A `#false` at tier 0 drops only
//!      itself. Every `#false` entry is itself dropped.
//!    - Marked entries (`+` prefix) always emit at their own source
//!      position with their own value.
//!    - Unmarked entries fall into single mode when *every* tier
//!      contributes ≤ 1 unmarked survivor (one occurrence emits at
//!      the earliest source index, with the highest-tier value) or
//!      repeat mode otherwise (every unmarked occurrence emits at
//!      its own source index).
//! 6. Resolve env-var contributions on a parallel channel
//!    (`SPEC.md` §2.11 / §2.8.5): build per-tier env slices in
//!    ascending order (defaults, root, …, leaf). For each name,
//!    walk tiers descending and pick the first tier that has an
//!    outcome (Set or Unset); the per-name outcome is emitted at
//!    the first-occurrence walk position in the ascending order
//!    so `--list` and `--dry-run` orderings stay deterministic.
//!
//! Positionals are not subject to override or suppression and are
//! emitted at their walk position in source order.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use miette::SourceSpan;

use crate::config::{
    Argument, Command, CommandChild, Config, EnvEntry, EnvValue, FlagMode, FlagValue,
};
use crate::errors::{Error, Result};
use crate::suggest::{build_help, nearest};

/// Output of a successful resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The program to execute (the command's `name` field).
    pub program: String,
    /// The candidate list after walk + collapse + suppression.
    pub args: Vec<Argument>,
    /// Env-var outcomes to apply to the spawned child, in
    /// first-occurrence walk order (`SPEC.md` §2.11).
    pub env: Vec<EnvOp>,
    /// Effective working directory for the spawned child per
    /// `SPEC.md` §2.12.3. `None` means inherit `jig`'s working
    /// directory (no `cwd=` resolved). Relative `cwd=` values in
    /// the config have already been resolved against the config
    /// file's directory.
    pub cwd: Option<PathBuf>,
}

/// One env-var operation to apply to the spawned child process via
/// `Command::env` / `Command::env_remove`. Per `SPEC.md` §2.11 /
/// §3.6.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvOp {
    /// Call `Command::env(name, value)`.
    Set {
        /// Variable name.
        name: String,
        /// Value to assign on the child.
        value: String,
    },
    /// Call `Command::env_remove(name)`.
    Unset {
        /// Variable name.
        name: String,
    },
}

/// Trace of how a resolution arrived at its argv, env, and cwd.
/// Returned by [`resolve_with_trace`] alongside [`Resolved`] and
/// consumed by [`crate::explain`] to render `--explain` output.
///
/// One [`SegmentTrace`] is produced for each emitted item in
/// [`Resolved::args`]; the index correspondence is positional.
/// Likewise one [`EnvTrace`] per [`EnvOp`] in [`Resolved::env`].
#[derive(Debug, Clone)]
pub struct ResolutionTrace {
    /// Alias of the matched command, if any (e.g. `"serve"`).
    pub alias: Option<String>,
    /// Selected profile name (the leaf of the inheritance chain).
    pub selected_profile: Option<String>,
    /// Inheritance chain `[root, ..., leaf]`. Empty when no profile
    /// is selected.
    pub chain: Vec<String>,
    /// Per-emitted-argv-segment story, parallel to [`Resolved::args`].
    pub segments: Vec<SegmentTrace>,
    /// Keys whose every candidate was suppressed (no argv emission).
    /// Surfaced by the renderer so vanished flags are explainable.
    pub suppressed: Vec<SuppressedKey>,
    /// Per-env-var story, parallel to [`Resolved::env`].
    pub env: Vec<EnvTrace>,
    /// cwd resolution outcome.
    pub cwd: CwdTrace,
}

/// Story of one emitted argv item. The associated [`Resolved::args`]
/// entry (looked up by index) carries the rendered tokens; this
/// struct only carries the merge story.
#[derive(Debug, Clone)]
pub struct SegmentTrace {
    /// One-line description of the merge decision, e.g.
    /// `"single-mode merge (<=1 unmarked per tier)"`,
    /// `"repeat mode (this is one of several emissions)"`, or
    /// `"marker (+) — emits at own position"`. `None` for the trivial
    /// single-contributor case so the renderer can skip it.
    pub mode_summary: Option<String>,
    /// Tiers that won part of the emission (position, value, or both).
    /// In source-order by tier so the renderer can pair `position
    /// from` and `value from` lines without reordering.
    pub contributors: Vec<Contributor>,
    /// Negative-space facts to surface for this segment: middle-tier
    /// losses in 3+ tier chains, and `#false` cleared entries that
    /// the renderer wants to display alongside an emission.
    pub dropped: Vec<DroppedInfo>,
}

/// A whole key that was suppressed and produced no argv emission.
/// The suppressing `#false` itself is included in [`Self::cleared`]
/// (with reason [`DroppedReason::SelfFalse`]) so the renderer can
/// list every entry contributing to the key in one place.
#[derive(Debug, Clone)]
pub struct SuppressedKey {
    /// Resolved CLI form (e.g. `"--timeout"`).
    pub key: String,
    /// Entries that were cleared, in source order — includes both
    /// the cleared lower-tier values and the `#false` suppressor.
    pub cleared: Vec<DroppedInfo>,
}

/// One tier's contribution to an emitted segment.
#[derive(Debug, Clone)]
pub struct Contributor {
    /// Role this tier played in producing the emission.
    pub role: ContributorRole,
    /// Tier index: 0 = defaults, 1..=N = inheritance-chain index.
    pub tier: usize,
    /// `"defaults"` for tier 0, profile name otherwise.
    pub tier_label: String,
    /// Source position of the contributing entry, when known. `None`
    /// for positional arguments (the parser does not currently track
    /// spans for positionals).
    pub span: Option<SourceSpan>,
    /// True when this tier is an ancestor in the inheritance chain
    /// (not the selected leaf, not defaults). Renders as
    /// `"(inherited)"` in `--explain` output.
    pub inherited: bool,
    /// True when this contributor is a `#null` ghost — it supplied
    /// a position but no value (`SPEC.md` §2.4.3). The renderer adds
    /// a `(#null ghost — no value)` hint so the user can tell a
    /// ghost-position-winner from a regular default.
    pub ghost: bool,
}

/// What part of an emission a contributor supplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContributorRole {
    /// Only contributor to this segment; no winner-talk needed.
    /// Used both for trivially-single-source emissions and for
    /// single-mode merges where the same tier supplied both
    /// position and value.
    Sole,
    /// Single-mode merge — this tier supplied position only.
    PositionOnly,
    /// Single-mode merge — this tier supplied value only.
    ValueOnly,
    /// `+`-marked entry, always emits at own position.
    Marker,
    /// One emission of a repeat-mode group.
    Repeat,
}

/// One entry that was dropped during resolution.
#[derive(Debug, Clone)]
pub struct DroppedInfo {
    /// Tier index of the dropped entry.
    pub tier: usize,
    /// `"defaults"` for tier 0, profile name otherwise.
    pub tier_label: String,
    /// Source position of the dropped entry.
    pub span: SourceSpan,
    /// Rendered value as it appeared in the config, e.g. `"\"x\""`,
    /// `"#false"`, `"#null"`.
    pub rendered_value: String,
    /// Why this entry didn't reach argv.
    pub reason: DroppedReason,
}

/// Why a `DroppedInfo` entry was dropped.
#[derive(Debug, Clone)]
pub enum DroppedReason {
    /// Cleared by a `#false` at a higher tier; the suppressing
    /// `#false`'s tier and position are named so the renderer can
    /// point at it.
    SuppressedByFalse {
        /// Tier label of the suppressing `#false`.
        by_tier_label: String,
        /// Source position of the suppressing `#false`.
        by_span: SourceSpan,
    },
    /// Lost both position and value to a higher tier in a single-mode
    /// merge across 3+ tiers (a middle tier with no role).
    MiddleTierLost,
    /// `#false` entry itself (which never emits regardless of tier).
    SelfFalse,
}

/// Story of one resolved env-var outcome.
#[derive(Debug, Clone)]
pub struct EnvTrace {
    /// Resolved outcome (parallel to [`Resolved::env`]). The
    /// variable name lives inside the `outcome`.
    pub outcome: EnvOp,
    /// Tier that supplied the winning outcome.
    pub winner_tier: usize,
    /// `"defaults"` for tier 0, profile name otherwise.
    pub winner_tier_label: String,
    /// Source position of the winning entry.
    pub winner_span: SourceSpan,
    /// True when the winning tier is a chain ancestor.
    pub winner_inherited: bool,
    /// Lower-tier contributions that were shadowed.
    pub shadowed: Vec<EnvShadowed>,
}

/// One lower-tier env-var contribution that lost to a higher tier.
#[derive(Debug, Clone)]
pub struct EnvShadowed {
    /// Tier index.
    pub tier: usize,
    /// `"defaults"` for tier 0, profile name otherwise.
    pub tier_label: String,
    /// Source position.
    pub span: SourceSpan,
    /// The value at that tier (`Set("...")` or `Unset`).
    pub value: EnvValue,
}

/// Outcome of cwd resolution.
#[derive(Debug, Clone)]
pub enum CwdTrace {
    /// No `cwd=` resolved; child inherits `jig`'s working directory.
    Inherited,
    /// Resolved from a `cwd=` somewhere in the config.
    Resolved {
        /// Source-text value as written.
        source: String,
        /// Final absolute path after anchoring.
        resolved: PathBuf,
        /// Tier index that supplied the value (0 = command-level,
        /// 1..=N = inheritance-chain index).
        tier: usize,
        /// `"command"` for tier 0, profile name otherwise.
        tier_label: String,
        /// Source position of the `cwd=` value.
        span: SourceSpan,
        /// True when the tier is a chain ancestor.
        inherited: bool,
    },
}

/// Resolve `name` and optional `profile` against `config`.
///
/// `config_dir` is the directory containing the loaded config file
/// (provided by [`crate::config::load`]). It is used as the anchor
/// for relative `cwd=` values per `SPEC.md` §2.12.2.
///
/// # Errors
///
/// Returns [`Error::UnknownCommand`] if no command matches the
/// name or alias, or [`Error::UnknownProfile`] if `profile` does
/// not exist on the matched command.
pub fn resolve(
    config: &Config,
    name: &str,
    profile: Option<&str>,
    config_dir: &Path,
) -> Result<Resolved> {
    let (resolved, _) = resolve_inner(config, name, profile, config_dir, false)?;
    Ok(resolved)
}

/// Resolve `name` and optional `profile` and return a [`Resolved`]
/// plus a [`ResolutionTrace`] describing how each emitted argument,
/// env-var, and cwd outcome was reached. The trace fuels
/// `--explain` output.
///
/// The trace adds bookkeeping work — small but non-zero — over
/// [`resolve`]. Hot paths that do not need the trace (`exec::run`,
/// `--dry-run`) should keep using [`resolve`].
///
/// # Errors
///
/// Same as [`resolve`].
pub fn resolve_with_trace(
    config: &Config,
    name: &str,
    profile: Option<&str>,
    config_dir: &Path,
) -> Result<(Resolved, ResolutionTrace)> {
    let (resolved, trace) = resolve_inner(config, name, profile, config_dir, true)?;
    Ok((
        resolved,
        trace.expect("invariant: trace requested but not produced"),
    ))
}

/// Shared body for [`resolve`] / [`resolve_with_trace`]. When
/// `want_trace` is `false` the optional trace stays empty and no
/// bookkeeping work is done.
#[allow(clippy::too_many_lines)] // The merge + trace assembly belong together.
fn resolve_inner(
    config: &Config,
    name: &str,
    profile: Option<&str>,
    config_dir: &Path,
    want_trace: bool,
) -> Result<(Resolved, Option<ResolutionTrace>)> {
    let cmd = lookup_command(config, name)?;
    let selected_profile = match profile {
        None => None,
        Some(p) => Some(lookup_profile(cmd, p)?),
    };

    // Step 3 (module doc): build the inheritance chain for the
    // selected leaf. `chain[i]` is the i-th ancestor (0 = root,
    // last = leaf). Tier assignment is `i + 1`, so defaults stay at
    // tier 0 and the leaf sits at the highest tier. With no profile
    // selected the chain is empty and only defaults activate.
    let chain: Vec<&str> =
        selected_profile.map_or_else(Vec::new, |leaf| inheritance_chain(cmd, leaf));
    let tier_of: HashMap<&str, usize> =
        chain.iter().enumerate().map(|(i, &n)| (n, i + 1)).collect();

    // Step 4: walk children, tagging each candidate with its origin
    // tier. Defaults are tier 0; profiles in the chain emit their
    // body candidates at the matching tier. Other profiles are
    // skipped (§2.7). Profile-name uniqueness within a command (§2.9)
    // means each chain profile matches at most one child; without
    // that invariant, two children sharing a name would both push.
    let mut candidates: Vec<(Argument, usize /* tier */)> = Vec::new();
    for child in &cmd.children {
        match child {
            CommandChild::Default(arg) => candidates.push((arg.clone(), 0)),
            CommandChild::Profile { name, args, .. } if tier_of.contains_key(name.as_str()) => {
                let tier = tier_of[name.as_str()];
                for arg in args {
                    candidates.push((arg.clone(), tier));
                }
            }
            CommandChild::Profile { .. } => {}
        }
    }

    // Step 5: group flag candidates by resolved CLI form, then
    // compute the emission plan per key. The plan records, for each
    // emitting source index, the value to use at that position;
    // indices not present are suppressed (whether by collapse or by
    // `#false`).
    let mut by_key: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, (arg, _)) in candidates.iter().enumerate() {
        if let Argument::Flag { key, .. } = arg {
            by_key.entry(key.to_cli_flag()).or_default().push(i);
        }
    }
    let mut emit_value: HashMap<usize, FlagValue> = HashMap::new();
    // For tracing: per-key plan info, keyed by resolved CLI form.
    // `None` on the cheap (non-trace) path so plan_key skips the
    // bookkeeping entirely.
    let mut key_traces: Option<HashMap<String, KeyPlanTrace>> = want_trace.then(HashMap::new);
    for (key, indices) in &by_key {
        let mut trace_slot = want_trace.then(KeyPlanTrace::default);
        plan_key(&candidates, indices, &mut emit_value, trace_slot.as_mut());
        if let (Some(t), Some(map)) = (trace_slot, key_traces.as_mut()) {
            map.insert(key.clone(), t);
        }
    }

    // Step 6: resolve env-var contributions on a parallel channel.
    // Tier ordering mirrors the argv side: defaults at tier 0, then
    // each ancestor's env in chain order, then the leaf's env.
    let mut per_tier_env: Vec<&[EnvEntry]> = vec![cmd.env.as_slice()];
    for &profile_name in &chain {
        let env_slice = cmd
            .children
            .iter()
            .find_map(|c| match c {
                CommandChild::Profile { name, env, .. } if name == profile_name => {
                    Some(env.as_slice())
                }
                _ => None,
            })
            .expect("invariant: chain entries reference defined profiles");
        per_tier_env.push(env_slice);
    }
    let env = resolve_env(&per_tier_env);

    // Assemble: positionals always emit; flags emit iff they appear
    // in the plan, taking the planned value (which is what makes
    // profile override work in single mode). The trace path also
    // stashes the per-emission candidate-walk index and the key
    // string so the trace builder below can recover each segment's
    // origin without re-walking the candidate list; the no-trace
    // path skips both, matching the pre-feature allocation profile.
    let mut out: Vec<Argument> = Vec::with_capacity(candidates.len());
    let mut emit_walk_idx: Option<Vec<usize>> =
        want_trace.then(|| Vec::with_capacity(candidates.len()));
    let mut emit_keys: Option<Vec<Option<String>>> =
        want_trace.then(|| Vec::with_capacity(candidates.len()));
    for (i, (arg, _)) in candidates.iter().enumerate() {
        match arg {
            Argument::Positional(_) => {
                out.push(arg.clone());
                if let Some(idxs) = emit_walk_idx.as_mut() {
                    idxs.push(i);
                }
                if let Some(keys) = emit_keys.as_mut() {
                    keys.push(None);
                }
            }
            Argument::Flag {
                key,
                key_span,
                mode,
                ..
            } => {
                if let Some(value) = emit_value.remove(&i) {
                    out.push(Argument::Flag {
                        key: key.clone(),
                        key_span: *key_span,
                        value,
                        mode: *mode,
                    });
                    if let Some(idxs) = emit_walk_idx.as_mut() {
                        idxs.push(i);
                    }
                    if let Some(keys) = emit_keys.as_mut() {
                        keys.push(Some(key.to_cli_flag()));
                    }
                }
            }
        }
    }

    let cwd = effective_cwd(cmd, &chain, config_dir);

    let trace = if want_trace {
        let key_traces = key_traces.expect("invariant: want_trace ⇒ key_traces is Some");
        let emit_walk_idx = emit_walk_idx.expect("invariant: want_trace ⇒ emit_walk_idx is Some");
        let emit_keys = emit_keys.expect("invariant: want_trace ⇒ emit_keys is Some");
        let cwd_trace = build_cwd_trace(cmd, &chain, config_dir);
        let env_trace = build_env_trace(&per_tier_env, &chain);
        let (segments, suppressed) = build_segment_traces(
            &candidates,
            &chain,
            &by_key,
            &key_traces,
            &out,
            &emit_walk_idx,
            &emit_keys,
        );
        Some(ResolutionTrace {
            alias: cmd.alias.clone(),
            selected_profile: selected_profile.map(str::to_string),
            chain: chain.iter().map(|s| (*s).to_string()).collect(),
            segments,
            suppressed,
            env: env_trace,
            cwd: cwd_trace,
        })
    } else {
        None
    };

    Ok((
        Resolved {
            program: cmd.name.clone(),
            args: out,
            env,
            cwd,
        },
        trace,
    ))
}

/// Compute the effective working directory per `SPEC.md` §2.12.3.
///
/// Walks the `extends` chain leaf → root looking for a profile-side
/// `cwd=`; falls back to the command's `cwd=` if no profile in the
/// chain supplied one. Relative values are resolved against
/// `config_dir`; absolute values are used as written.
fn effective_cwd(cmd: &Command, chain: &[&str], config_dir: &Path) -> Option<PathBuf> {
    // chain is ordered [root, …, leaf]; walk leaf-first.
    for profile_name in chain.iter().rev() {
        let cwd = cmd.children.iter().find_map(|c| match c {
            CommandChild::Profile { name, cwd, .. } if name == *profile_name => cwd.as_ref(),
            _ => None,
        });
        if let Some((path, _)) = cwd {
            return Some(resolve_cwd_path(path, config_dir));
        }
    }
    cmd.cwd
        .as_ref()
        .map(|(path, _)| resolve_cwd_path(path, config_dir))
}

/// Resolve a `cwd=` value to an absolute path. Absolute values are
/// used unchanged; relative values are joined onto `config_dir` per
/// `SPEC.md` §2.12.2. No symlink resolution or `..` normalisation is
/// performed — `chdir(2)` interprets the path at exec time.
fn resolve_cwd_path(value: &str, config_dir: &Path) -> PathBuf {
    let p = Path::new(value);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        config_dir.join(p)
    }
}

/// Resolve env-var outcomes per `SPEC.md` §2.11 / §2.8.5. The slices
/// in `per_tier_env` are ordered ascending: `[0]` is the command's
/// defaults, `[1]` is the root ancestor's env, `[N]` is the
/// selected leaf's env. For each distinct name in the concatenated
/// walk we emit one outcome at its first-occurrence position; the
/// outcome itself is decided by [`pick_env_outcome`] (descending
/// walk, highest tier wins).
fn resolve_env(per_tier_env: &[&[EnvEntry]]) -> Vec<EnvOp> {
    // First-occurrence ordering across the ascending concatenated
    // walk so `--list` / `--dry-run` output stays deterministic.
    let mut first_index: HashMap<&str, usize> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    for slice in per_tier_env {
        for entry in *slice {
            if !first_index.contains_key(entry.name.as_str()) {
                first_index.insert(entry.name.as_str(), order.len());
                order.push(entry.name.as_str());
            }
        }
    }

    let mut out: Vec<EnvOp> = Vec::with_capacity(order.len());
    for name in order {
        out.push(pick_env_outcome(name, per_tier_env));
    }
    out
}

/// Pick the winning outcome for one env-var name across all tiers.
/// Walks tiers descending (leaf → … → defaults); the first tier
/// that has an outcome for `name` wins. Per-scope uniqueness
/// (validation §2.9) guarantees at most one entry per name per
/// tier, so the in-slice search is unambiguous.
fn pick_env_outcome(name: &str, per_tier_env: &[&[EnvEntry]]) -> EnvOp {
    for slice in per_tier_env.iter().rev() {
        if let Some(entry) = slice.iter().find(|e| e.name == name) {
            return match &entry.value {
                EnvValue::Set(v) => EnvOp::Set {
                    name: name.to_string(),
                    value: v.clone(),
                },
                EnvValue::Unset => EnvOp::Unset {
                    name: name.to_string(),
                },
            };
        }
    }
    unreachable!("invariant: name was added to `order` from one of the tier slices")
}

/// Per-key resolution per `SPEC.md` §2.8 / §2.8.5. Reads the
/// candidates at `indices` (all sharing a resolved CLI key), applies
/// the N-tier suppression / mode / value / position rules, and
/// writes the resulting emission decisions into `emit_value`.
///
/// Each candidate carries a `tier` index: 0 for defaults, 1 for the
/// root ancestor, …, N for the selected leaf. With no inheritance
/// the rules degenerate to the two-tier algorithm; the existing
/// merge tests verify that byte-for-byte.
///
/// When `trace` is `Some`, the merge decision is recorded for the
/// caller (used by [`resolve_with_trace`]); when `None`, no trace
/// work is performed so the existing `resolve` hot path keeps the
/// same allocation profile it had before tracing was added.
//
// The tracing side-effects sit inline with the merge stages
// (suppression → marker → mode selection → emission) because each
// stage's decision feeds the next. Pulling them into helpers would
// require threading the same five locals through extra function
// boundaries with no clarity gain.
//
// `as_deref_mut` is needed (not redundant) because `trace` is
// re-borrowed across multiple stage blocks; consuming it on the
// first stage would leave nothing for the rest.
#[allow(clippy::too_many_lines, clippy::needless_option_as_deref)]
fn plan_key(
    candidates: &[(Argument, usize)],
    indices: &[usize],
    emit_value: &mut HashMap<usize, FlagValue>,
    mut trace: Option<&mut KeyPlanTrace>,
) {
    struct Entry<'a> {
        idx: usize,
        tier: usize,
        value: &'a FlagValue,
        mode: FlagMode,
    }
    let entries: Vec<Entry<'_>> = indices
        .iter()
        .map(|&i| {
            let (arg, tier) = &candidates[i];
            let Argument::Flag { value, mode, .. } = arg else {
                unreachable!("invariant: by_key only references flag candidates");
            };
            Entry {
                idx: i,
                tier: *tier,
                value,
                mode: *mode,
            }
        })
        .collect();

    // Suppression. The highest tier (T > 0) carrying a `#false` for
    // this key clears every entry at tiers `< T`; in addition every
    // `#false` entry is dropped, regardless of tier. A `#false` at
    // tier 0 alone clears only itself. `#null` placeholders
    // (§2.4.3) are never survivors — they have no value to emit and
    // do not trigger suppression — but their source positions feed
    // the single-mode position pool below.
    let max_false_tier: Option<usize> = entries
        .iter()
        .filter(|e| e.tier > 0 && matches!(e.value, FlagValue::Bool(false)))
        .map(|e| e.tier)
        .max();
    let surviving: Vec<&Entry<'_>> = entries
        .iter()
        .filter(|e| {
            if matches!(e.value, FlagValue::Bool(false) | FlagValue::Null) {
                return false;
            }
            if let Some(t) = max_false_tier
                && e.tier < t
            {
                return false;
            }
            true
        })
        .collect();

    if let Some(t) = trace.as_deref_mut() {
        t.max_false_tier = max_false_tier;
        if let Some(tt) = max_false_tier {
            t.suppression_sources = entries
                .iter()
                .filter(|e| e.tier == tt && matches!(e.value, FlagValue::Bool(false)))
                .map(|e| e.idx)
                .collect();
        }
        for e in &entries {
            let fate = if matches!(e.value, FlagValue::Bool(false)) {
                CandidateFate::SelfFalse
            } else if matches!(e.value, FlagValue::Null) {
                // Updated below once we know if the ghost's position
                // was picked.
                CandidateFate::NullGhostUnused
            } else if let Some(tt) = max_false_tier
                && e.tier < tt
            {
                CandidateFate::SuppressedByFalse
            } else {
                CandidateFate::Pending
            };
            t.fates.insert(e.idx, fate);
        }
    }

    // Marked entries always emit at their own position, regardless
    // of unmarked-side mode. `#null` is rejected by the parser
    // when combined with `+`, so a marked survivor is never null.
    let mut any_marker = false;
    for e in surviving.iter().filter(|e| e.mode == FlagMode::Append) {
        emit_value.insert(e.idx, e.value.clone());
        any_marker = true;
        if let Some(t) = trace.as_deref_mut() {
            t.fates.insert(e.idx, CandidateFate::Marker);
        }
    }

    // Unmarked entries decide single-mode vs repeat-mode.
    let unmarked: Vec<&&Entry<'_>> = surviving
        .iter()
        .filter(|e| e.mode == FlagMode::Plain)
        .collect();
    if unmarked.is_empty() {
        if let Some(t) = trace.as_deref_mut() {
            t.mode = if any_marker {
                KeyMergeMode::MarkerOnly
            } else {
                KeyMergeMode::AllSuppressed
            };
        }
        return;
    }

    // Single mode iff every tier contributes ≤ 1 unmarked survivor.
    // For the 2-tier case (defaults + one profile) this matches the
    // current `|D| ≤ 1 && |P| ≤ 1` predicate exactly.
    let mut per_tier_count: HashMap<usize, usize> = HashMap::new();
    for e in &unmarked {
        *per_tier_count.entry(e.tier).or_insert(0) += 1;
    }
    let single_mode = per_tier_count.values().all(|&c| c <= 1);

    if single_mode {
        // Emit one occurrence: at the earliest source index across
        // all unmarked survivors AND any `#null` ghosts that
        // weren't cleared by the T-cascade. Per §2.4.3, a `#null`
        // contributes only its source position to the first-
        // occurrence pool; it never supplies a value. Value comes
        // from the highest-tier survivor.
        let ghost_idxs: Vec<usize> = entries
            .iter()
            .filter(|e| matches!(e.value, FlagValue::Null))
            .filter(|e| max_false_tier.is_none_or(|t| e.tier >= t))
            .map(|e| e.idx)
            .collect();
        let pos_idx = unmarked
            .iter()
            .map(|e| e.idx)
            .chain(ghost_idxs.iter().copied())
            .min()
            .expect("invariant: unmarked is non-empty");
        let value_winner = unmarked
            .iter()
            .max_by_key(|e| e.tier)
            .expect("invariant: unmarked is non-empty");
        let value = value_winner.value.clone();
        let value_winner_idx = value_winner.idx;
        emit_value.insert(pos_idx, value);

        if let Some(t) = trace.as_deref_mut() {
            t.mode = if any_marker {
                KeyMergeMode::SingleAndMarker
            } else {
                KeyMergeMode::Single
            };
            // Update ghost fate: if the position winner is a ghost
            // (i.e. its candidate idx is among ghost_idxs), record
            // that. Otherwise the ghosts stay marked Unused.
            if ghost_idxs.contains(&pos_idx) {
                t.fates
                    .insert(pos_idx, CandidateFate::NullGhostPositionUsed);
            }
            // Decide role of each unmarked survivor.
            for e in &unmarked {
                let role = if e.idx == pos_idx && e.idx == value_winner_idx {
                    CandidateFate::SinglePositionAndValue
                } else if e.idx == pos_idx {
                    CandidateFate::SinglePositionOnly { value_winner_idx }
                } else if e.idx == value_winner_idx {
                    // Position came from another candidate (or a ghost).
                    CandidateFate::SingleValueOnly {
                        position_winner_idx: pos_idx,
                    }
                } else {
                    CandidateFate::LostMiddle
                };
                t.fates.insert(e.idx, role);
            }
        }
    } else {
        // Repeat mode: every unmarked occurrence emits in place.
        for e in &unmarked {
            emit_value.insert(e.idx, e.value.clone());
            if let Some(t) = trace.as_deref_mut() {
                t.fates.insert(e.idx, CandidateFate::Repeat);
            }
        }
        if let Some(t) = trace.as_deref_mut() {
            t.mode = if any_marker {
                KeyMergeMode::RepeatAndMarker
            } else {
                KeyMergeMode::Repeat
            };
        }
    }
}

/// Internal: per-key merge decision, collected when [`plan_key`] is
/// invoked with a trace slot. Records enough detail for the renderer
/// to reconstruct contributors, dropped entries, and mode summaries
/// without re-running the merge logic.
#[derive(Debug, Default)]
struct KeyPlanTrace {
    /// Resolved mode (`Single`, `Repeat`, `MarkerOnly`, mixed, or
    /// `AllSuppressed`).
    mode: KeyMergeMode,
    /// `max_false_tier` from the suppression cascade, if any.
    max_false_tier: Option<usize>,
    /// Candidate indices of the `#false` entries at
    /// `tier == max_false_tier` (the ones that triggered the
    /// cascade).
    suppression_sources: Vec<usize>,
    /// Per-candidate fate keyed by walk index.
    fates: HashMap<usize, CandidateFate>,
}

/// Overall mode the merge resolved to for one key.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum KeyMergeMode {
    /// No emissions; either every entry is `#false` / fully
    /// suppressed, or the key group is empty.
    #[default]
    AllSuppressed,
    /// One unmarked emission, possibly synthesised from multiple
    /// tiers (position from earliest, value from highest).
    Single,
    /// Multiple unmarked emissions (some tier had ≥ 2 survivors).
    Repeat,
    /// Only `+`-marked entries survived; no unmarked emission.
    MarkerOnly,
    /// Unmarked single-mode emission plus one or more marker (`+`)
    /// emissions in the same key group.
    SingleAndMarker,
    /// Unmarked repeat-mode emissions plus one or more marker (`+`)
    /// emissions in the same key group.
    RepeatAndMarker,
}

/// Per-candidate decision recorded by [`plan_key`] when tracing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidateFate {
    /// Placeholder during the trace build — should never escape.
    Pending,
    /// `#false` entry; itself never emits.
    SelfFalse,
    /// Suppressed by a higher-tier `#false`.
    SuppressedByFalse,
    /// `#null` ghost whose position was not picked.
    NullGhostUnused,
    /// `#null` ghost whose position became the single-mode emit point.
    NullGhostPositionUsed,
    /// `+`-marked entry, emits at its own position.
    Marker,
    /// Single mode: sole emitter — supplied both position and value.
    SinglePositionAndValue,
    /// Single mode: this candidate's position was picked, value came
    /// from another tier (held in `value_winner_idx`).
    SinglePositionOnly { value_winner_idx: usize },
    /// Single mode: this candidate's value won, position came from
    /// another candidate (held in `position_winner_idx`).
    SingleValueOnly { position_winner_idx: usize },
    /// Single mode: this candidate lost both position and value
    /// (middle tier in a 3+ tier merge).
    LostMiddle,
    /// Repeat mode: one of N unmarked emissions; emits at own idx.
    Repeat,
}

/// Stringify a tier index against the inheritance chain: `"defaults"`
/// for tier 0, otherwise the profile name at the matching chain
/// index (1-based). `chain` is `[root, …, leaf]`.
fn tier_label(tier: usize, chain: &[&str]) -> String {
    if tier == 0 {
        "defaults".to_string()
    } else {
        chain[tier - 1].to_string()
    }
}

/// Whether a tier is a chain ancestor (not defaults, not the leaf).
/// Used to decide whether to render the `(inherited)` annotation.
const fn is_inherited(tier: usize, chain_len: usize) -> bool {
    tier > 0 && tier < chain_len
}

/// Render a [`FlagValue`] back to the literal-ish form it took in
/// the config, for display in `--explain` drop lines and footnotes.
/// Used purely for human-readable output — never for argv emission.
fn render_flag_value(v: &FlagValue) -> String {
    match v {
        FlagValue::Bool(true) => "#true".to_string(),
        FlagValue::Bool(false) => "#false".to_string(),
        FlagValue::Null => "#null".to_string(),
        // Quote literals so the user can tell `"123"` (a string) from
        // `123` (a number) in the trace; the difference doesn't show
        // up in argv but it's relevant when reading the config.
        FlagValue::Literal(s) => format!("\"{s}\""),
    }
}

/// Build a [`CwdTrace`] mirroring [`effective_cwd`]'s walk. Returns
/// `CwdTrace::Inherited` when nothing in the config supplies a `cwd=`.
fn build_cwd_trace(cmd: &Command, chain: &[&str], config_dir: &Path) -> CwdTrace {
    // Walk leaf-first; first profile with a `cwd=` wins.
    for profile_name in chain.iter().rev() {
        let position = chain.iter().position(|p| p == profile_name);
        let cwd = cmd.children.iter().find_map(|c| match c {
            CommandChild::Profile { name, cwd, .. } if name == *profile_name => cwd.as_ref(),
            _ => None,
        });
        if let Some((path, span)) = cwd {
            let resolved = resolve_cwd_path(path, config_dir);
            let tier = position
                .expect("invariant: profile_name comes from chain so it has a position")
                + 1;
            return CwdTrace::Resolved {
                source: path.clone(),
                resolved,
                tier,
                tier_label: (*profile_name).to_string(),
                span: *span,
                inherited: is_inherited(tier, chain.len()),
            };
        }
    }
    if let Some((path, span)) = cmd.cwd.as_ref() {
        let resolved = resolve_cwd_path(path, config_dir);
        return CwdTrace::Resolved {
            source: path.clone(),
            resolved,
            tier: 0,
            tier_label: "command".to_string(),
            span: *span,
            inherited: false,
        };
    }
    CwdTrace::Inherited
}

/// Build the per-env-var trace mirror of [`resolve_env`]'s output.
/// For each variable name the winner is the highest tier with an
/// outcome; lower-tier contributions for the same name become
/// `shadowed` entries in tier order.
fn build_env_trace(per_tier_env: &[&[EnvEntry]], chain: &[&str]) -> Vec<EnvTrace> {
    // Names in first-occurrence order across the ascending walk —
    // matches the order produced by `resolve_env` so the trace is
    // parallel to `Resolved::env`.
    let mut first_index: HashMap<&str, usize> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    for slice in per_tier_env {
        for entry in *slice {
            if !first_index.contains_key(entry.name.as_str()) {
                first_index.insert(entry.name.as_str(), order.len());
                order.push(entry.name.as_str());
            }
        }
    }

    order
        .iter()
        .map(|name| {
            // Find every tier that has an outcome for `name`, in
            // ascending tier order. The last one is the winner.
            let mut all: Vec<(usize, &EnvEntry)> = Vec::new();
            for (tier, slice) in per_tier_env.iter().enumerate() {
                if let Some(entry) = slice.iter().find(|e| e.name == *name) {
                    all.push((tier, entry));
                }
            }
            let &(winner_tier, winner_entry) = all
                .last()
                .expect("invariant: name came from one of the tier slices");
            let outcome = match &winner_entry.value {
                EnvValue::Set(v) => EnvOp::Set {
                    name: (*name).to_string(),
                    value: v.clone(),
                },
                EnvValue::Unset => EnvOp::Unset {
                    name: (*name).to_string(),
                },
            };
            let shadowed: Vec<EnvShadowed> = all
                .iter()
                .take(all.len().saturating_sub(1))
                .map(|(tier, entry)| EnvShadowed {
                    tier: *tier,
                    tier_label: tier_label(*tier, chain),
                    span: entry.name_span,
                    value: entry.value.clone(),
                })
                .collect();
            EnvTrace {
                outcome,
                winner_tier,
                winner_tier_label: tier_label(winner_tier, chain),
                winner_span: winner_entry.name_span,
                winner_inherited: is_inherited(winner_tier, chain.len()),
                shadowed,
            }
        })
        .collect()
}

/// Build the per-segment trace and the per-key suppression list
/// from the post-merge state. `out` is the assembled argv,
/// `emit_walk_idx[k]` is the candidate walk-index that produced
/// `out[k]`, and `emit_keys[k]` is the resolved CLI form for flag
/// emissions (or `None` for positionals).
fn build_segment_traces(
    candidates: &[(Argument, usize)],
    chain: &[&str],
    by_key: &HashMap<String, Vec<usize>>,
    key_traces: &HashMap<String, KeyPlanTrace>,
    out: &[Argument],
    emit_walk_idx: &[usize],
    emit_keys: &[Option<String>],
) -> (Vec<SegmentTrace>, Vec<SuppressedKey>) {
    let mut segments: Vec<SegmentTrace> = Vec::with_capacity(out.len());
    // Track per-key whether we've already attached its dropped
    // entries to an emission. In repeat mode several emissions share
    // a key; we attach drops to the first only to avoid duplication.
    let mut drops_attached: std::collections::HashSet<String> = std::collections::HashSet::new();

    for k in 0..out.len() {
        let walk_idx = emit_walk_idx[k];
        match &emit_keys[k] {
            None => {
                let tier = candidates[walk_idx].1;
                segments.push(SegmentTrace {
                    mode_summary: None,
                    contributors: vec![Contributor {
                        role: ContributorRole::Sole,
                        tier,
                        tier_label: tier_label(tier, chain),
                        span: None,
                        inherited: is_inherited(tier, chain.len()),
                        // Positionals never come from `#null` ghosts.
                        ghost: false,
                    }],
                    dropped: vec![],
                });
            }
            Some(key) => {
                let key_trace = &key_traces[key];
                let indices = &by_key[key];
                let attach_drops = drops_attached.insert(key.clone());
                let segment = build_flag_segment_trace(
                    walk_idx,
                    key_trace,
                    candidates,
                    indices,
                    chain,
                    attach_drops,
                );
                segments.push(segment);
            }
        }
    }

    let suppressed = build_suppressed_keys(candidates, chain, by_key, key_traces);
    (segments, suppressed)
}

/// Build the suppressed-keys list: every key whose candidates all
/// resolved to no argv emission (`AllSuppressed`). Sorted by the
/// earliest source span among the cleared entries so the renderer
/// presents keys in config order.
fn build_suppressed_keys(
    candidates: &[(Argument, usize)],
    chain: &[&str],
    by_key: &HashMap<String, Vec<usize>>,
    key_traces: &HashMap<String, KeyPlanTrace>,
) -> Vec<SuppressedKey> {
    let mut suppressed: Vec<SuppressedKey> = Vec::new();
    for (key, indices) in by_key {
        let key_trace = &key_traces[key];
        // Only keys whose entire group resolved to no argv emission
        // belong here. `MarkerOnly`, `SingleAndMarker`, and
        // `RepeatAndMarker` all emit at least one segment, so their
        // story is already in `SegmentTrace::dropped`; the other
        // `Single` / `Repeat` modes emit by definition. A new
        // `KeyMergeMode` variant would need to be classified
        // explicitly here.
        if !matches!(key_trace.mode, KeyMergeMode::AllSuppressed) {
            continue;
        }
        let (by_tier_label, by_span) = suppression_source(key_trace, candidates, chain, indices);
        let mut cleared: Vec<DroppedInfo> = Vec::new();
        for &i in indices {
            let (arg, tier) = &candidates[i];
            let Argument::Flag {
                value, key_span, ..
            } = arg
            else {
                unreachable!("invariant: by_key only references flag candidates");
            };
            let reason = match key_trace.fates.get(&i).copied() {
                Some(CandidateFate::SelfFalse) => DroppedReason::SelfFalse,
                Some(CandidateFate::SuppressedByFalse) => DroppedReason::SuppressedByFalse {
                    by_tier_label: by_tier_label.clone(),
                    by_span,
                },
                _ => continue,
            };
            cleared.push(DroppedInfo {
                tier: *tier,
                tier_label: tier_label(*tier, chain),
                span: *key_span,
                rendered_value: render_flag_value(value),
                reason,
            });
        }
        // A key whose only entries are `#null` ghosts resolves to
        // `AllSuppressed` (ghosts never survive the unmarked filter
        // and never trigger suppression), but no actual suppression
        // happened — the user just declared a placeholder that no
        // profile filled. Skip these so the renderer doesn't print
        // a misleading "suppressed: --a" stub with no rows.
        if cleared.is_empty() {
            continue;
        }
        suppressed.push(SuppressedKey {
            key: key.clone(),
            cleared,
        });
    }
    // `by_key` is a HashMap, so the loop above produces suppressed
    // entries in non-deterministic order. Sort by the earliest
    // source span among cleared entries so the renderer's output
    // tracks the order the user wrote the keys in the config.
    suppressed.sort_by_key(|s| {
        s.cleared
            .iter()
            .map(|d| d.span.offset())
            .min()
            .unwrap_or(usize::MAX)
    });
    suppressed
}

/// Find the suppressing `#false` for a key: the highest-tier `#false`
/// when one exists, falling back to a tier-0 self-`#false` (whose
/// own clearing is the only fact to report). Returns the source
/// position and tier label of that entry.
fn suppression_source(
    key_trace: &KeyPlanTrace,
    candidates: &[(Argument, usize)],
    chain: &[&str],
    indices: &[usize],
) -> (String, SourceSpan) {
    if key_trace.suppression_sources.is_empty() {
        // Tier-0 self-#false case: find any SelfFalse candidate so
        // the renderer has a span to point at.
        let sf_idx = indices
            .iter()
            .copied()
            .find(|&i| matches!(key_trace.fates.get(&i), Some(CandidateFate::SelfFalse)));
        sf_idx.map_or_else(
            || (String::new(), SourceSpan::from((0, 0))),
            |i| {
                let (tier, span) = flag_meta(candidates, i);
                (tier_label(tier, chain), span)
            },
        )
    } else {
        let i = key_trace.suppression_sources[0];
        let (tier, span) = flag_meta(candidates, i);
        (tier_label(tier, chain), span)
    }
}

/// Build a [`SegmentTrace`] for one flag emission.
///
/// `walk_idx` is the candidate index that emitted (i.e. the entry
/// stored in [`Resolved::args`] for this segment). `attach_drops`
/// is `true` for the first emission of a given key in repeat mode
/// (so `dropped` and middle-tier losses are not duplicated across
/// sibling emissions); for the second and later emissions of the
/// same key, drops are intentionally left empty.
fn build_flag_segment_trace(
    walk_idx: usize,
    key_trace: &KeyPlanTrace,
    candidates: &[(Argument, usize)],
    indices: &[usize],
    chain: &[&str],
    attach_drops: bool,
) -> SegmentTrace {
    let fate = key_trace
        .fates
        .get(&walk_idx)
        .copied()
        .expect("invariant: emitted candidate has a recorded fate");

    let (mode_summary, contributors) =
        flag_contributors(walk_idx, fate, key_trace, candidates, chain);

    let dropped = if attach_drops {
        collect_segment_drops(key_trace, candidates, indices, chain)
    } else {
        Vec::new()
    };

    SegmentTrace {
        mode_summary,
        contributors,
        dropped,
    }
}

/// Pull a candidate's tier and key span out of the candidate list.
/// Encapsulates the `Argument::Flag` destructuring so callers stay
/// out of the unreachable-arm dance.
fn flag_meta(candidates: &[(Argument, usize)], idx: usize) -> (usize, SourceSpan) {
    let (arg, tier) = &candidates[idx];
    let Argument::Flag { key_span, .. } = arg else {
        unreachable!("invariant: by_key only references flag candidates");
    };
    (*tier, *key_span)
}

/// Build the contributor list (plus mode summary) for one emission
/// given its fate. Handles the trivial single-contributor cases
/// (`mode_summary = None`) as well as the multi-tier merges.
/// Mode summary surfaced for every single-mode merge that spans more
/// than one tier. Centralised so future wording tweaks are a one-
/// place edit.
const SINGLE_MODE_SUMMARY: &str = "single-mode merge (<=1 unmarked per tier)";

fn flag_contributors(
    walk_idx: usize,
    fate: CandidateFate,
    key_trace: &KeyPlanTrace,
    candidates: &[(Argument, usize)],
    chain: &[&str],
) -> (Option<String>, Vec<Contributor>) {
    let make = |role: ContributorRole, idx: usize| -> Contributor {
        let (tier, span) = flag_meta(candidates, idx);
        let ghost = matches!(
            candidates[idx].0,
            Argument::Flag {
                value: FlagValue::Null,
                ..
            }
        );
        Contributor {
            role,
            tier,
            tier_label: tier_label(tier, chain),
            span: Some(span),
            inherited: is_inherited(tier, chain.len()),
            ghost,
        }
    };

    match fate {
        CandidateFate::Marker => (
            Some("marker (+) — emits at own position".to_string()),
            vec![make(ContributorRole::Marker, walk_idx)],
        ),
        CandidateFate::Repeat => (
            Some("repeat mode — every unmarked occurrence emits at its own position".to_string()),
            vec![make(ContributorRole::Repeat, walk_idx)],
        ),
        CandidateFate::SinglePositionAndValue => {
            (None, vec![make(ContributorRole::Sole, walk_idx)])
        }
        CandidateFate::SinglePositionOnly { value_winner_idx } => (
            Some(SINGLE_MODE_SUMMARY.to_string()),
            vec![
                make(ContributorRole::PositionOnly, walk_idx),
                make(ContributorRole::ValueOnly, value_winner_idx),
            ],
        ),
        CandidateFate::SingleValueOnly {
            position_winner_idx,
        } => (
            Some(SINGLE_MODE_SUMMARY.to_string()),
            // List position before value so the renderer's
            // `position from / value from` order is determined by
            // the trace, not by the renderer.
            vec![
                make(ContributorRole::PositionOnly, position_winner_idx),
                make(ContributorRole::ValueOnly, walk_idx),
            ],
        ),
        CandidateFate::NullGhostPositionUsed => {
            // Position is a `#null` ghost; the value came from the
            // value winner — find it via the fates map.
            let value_winner_idx = key_trace
                .fates
                .iter()
                .find_map(|(idx, f)| match f {
                    CandidateFate::SingleValueOnly { .. }
                    | CandidateFate::SinglePositionAndValue => Some(*idx),
                    _ => None,
                })
                .unwrap_or(walk_idx);
            (
                Some(SINGLE_MODE_SUMMARY.to_string()),
                vec![
                    make(ContributorRole::PositionOnly, walk_idx),
                    make(ContributorRole::ValueOnly, value_winner_idx),
                ],
            )
        }
        // Defensive: should not be reached because non-emitting
        // fates do not produce a Resolved::args entry, so
        // build_segment_traces never reaches them.
        CandidateFate::Pending
        | CandidateFate::SelfFalse
        | CandidateFate::SuppressedByFalse
        | CandidateFate::NullGhostUnused
        | CandidateFate::LostMiddle => (None, vec![make(ContributorRole::Sole, walk_idx)]),
    }
}

/// Collect dropped/suppressed candidates for a key whose emission
/// is being traced. Used by `build_flag_segment_trace` for the first
/// (and only the first) emission per key.
fn collect_segment_drops(
    key_trace: &KeyPlanTrace,
    candidates: &[(Argument, usize)],
    indices: &[usize],
    chain: &[&str],
) -> Vec<DroppedInfo> {
    // Use the shared helper so the tier-0 self-`#false` fallback is
    // applied consistently with the suppressed-keys path.
    let (by_tier_label, by_span) = suppression_source(key_trace, candidates, chain, indices);

    let mut out: Vec<DroppedInfo> = Vec::new();
    for &i in indices {
        let (arg, tier) = &candidates[i];
        let Argument::Flag {
            value, key_span, ..
        } = arg
        else {
            unreachable!("invariant: by_key only references flag candidates");
        };
        let fate = key_trace.fates.get(&i).copied();
        let reason = match fate {
            Some(CandidateFate::SelfFalse) => DroppedReason::SelfFalse,
            Some(CandidateFate::SuppressedByFalse) => DroppedReason::SuppressedByFalse {
                by_tier_label: by_tier_label.clone(),
                by_span,
            },
            Some(CandidateFate::LostMiddle) => DroppedReason::MiddleTierLost,
            _ => continue,
        };
        out.push(DroppedInfo {
            tier: *tier,
            tier_label: tier_label(*tier, chain),
            span: *key_span,
            rendered_value: render_flag_value(value),
            reason,
        });
    }
    out
}

/// Build the inheritance chain for `leaf` in `cmd`. Returns
/// `[root, …, leaf]` by walking `extends` pointers upward. Per
/// `SPEC.md` §2.8.5, validation has already proven the graph is
/// acyclic and that every `extends` target resolves to a sibling
/// profile, so the walk terminates and never produces a stray
/// reference. We track the visited set and panic on a revisit so a
/// caller that bypasses validation gets a loud failure instead of an
/// infinite loop; the linear scan inside the loop also panics if a
/// parent name no longer resolves to a sibling profile.
fn inheritance_chain<'a>(cmd: &'a Command, leaf: &'a str) -> Vec<&'a str> {
    use std::collections::HashSet;
    let mut chain: Vec<&'a str> = vec![leaf];
    let mut visited: HashSet<&'a str> = HashSet::from([leaf]);
    let mut current: &'a str = leaf;
    loop {
        let parent = cmd.children.iter().find_map(|c| match c {
            CommandChild::Profile { name, extends, .. } if name == current => {
                extends.as_ref().map(|(p, _)| p.as_str())
            }
            _ => None,
        });
        match parent {
            Some(p) => {
                assert!(
                    cmd.children.iter().any(|c| matches!(
                        c,
                        CommandChild::Profile { name, .. } if name == p
                    )),
                    "invariant: validation rejects unknown `extends` parents before resolve runs"
                );
                assert!(
                    visited.insert(p),
                    "invariant: validation rejects inheritance cycles before resolve runs"
                );
                chain.push(p);
                current = p;
            }
            None => break,
        }
    }
    chain.reverse();
    chain
}

/// Look up a command for completion-candidate emission. Mirrors the
/// rules in [`lookup_command`] but never errors: returns `None` for
/// unknown names and for duplicated bare names (which have no unique
/// profile set — the user must invoke via an alias). Used by
/// [`crate::complete`].
#[must_use]
pub fn find_for_completion<'a>(config: &'a Config, name: &str) -> Option<&'a Command> {
    let name_matches: Vec<&Command> = config.commands.iter().filter(|c| c.name == name).collect();
    if name_matches.len() == 1 {
        return Some(name_matches[0]);
    }
    if name_matches.len() > 1 {
        // Duplicated bare name → ambiguous, no unique profile set.
        return None;
    }
    config
        .commands
        .iter()
        .find(|c| c.alias.as_deref() == Some(name))
}

fn lookup_command<'a>(config: &'a Config, name: &str) -> Result<&'a Command> {
    // Step 1: count name matches.
    let name_matches: Vec<&Command> = config.commands.iter().filter(|c| c.name == name).collect();

    if name_matches.len() == 1 {
        return Ok(name_matches[0]);
    }
    if name_matches.len() > 1 {
        // Validation guarantees every duplicated occurrence has an
        // alias; list them so the user can pick one.
        let aliases: Vec<&str> = name_matches
            .iter()
            .map(|c| {
                c.alias.as_deref().expect(
                    "invariant: validation requires every duplicated command name to have an alias",
                )
            })
            .collect();
        return Err(Error::AmbiguousCommand {
            name: name.to_string(),
            help: format!(
                "command name {name:?} appears more than once; invoke via one of its aliases: {}",
                aliases.join(", ")
            ),
        });
    }

    // Step 2: alias lookup. Validation enforces alias uniqueness so
    // at most one match is possible.
    if let Some(cmd) = config
        .commands
        .iter()
        .find(|c| c.alias.as_deref() == Some(name))
    {
        return Ok(cmd);
    }

    // Step 3: unknown. Build the did-you-mean candidate list from
    // names that are valid lookup keys (single-occurrence names) plus
    // every alias.
    let mut name_counts: HashMap<&str, usize> = HashMap::new();
    for cmd in &config.commands {
        *name_counts.entry(cmd.name.as_str()).or_insert(0) += 1;
    }
    let mut all: Vec<&str> = Vec::new();
    for cmd in &config.commands {
        if name_counts[cmd.name.as_str()] == 1 {
            all.push(&cmd.name);
        }
        if let Some(alias) = &cmd.alias {
            all.push(alias);
        }
    }
    let suggestion = nearest(name, &all);
    Err(Error::UnknownCommand {
        name: name.to_string(),
        help: build_help("commands", &all, suggestion),
    })
}

fn lookup_profile<'a>(cmd: &'a Command, profile: &'a str) -> Result<&'a str> {
    for child in &cmd.children {
        if let CommandChild::Profile { name, .. } = child
            && name == profile
        {
            return Ok(name);
        }
    }
    let available: Vec<&str> = cmd
        .children
        .iter()
        .filter_map(|c| match c {
            CommandChild::Profile { name, .. } => Some(name.as_str()),
            CommandChild::Default(_) => None,
        })
        .collect();
    let suggestion = nearest(profile, &available);
    Err(Error::UnknownProfile {
        profile: profile.to_string(),
        command: cmd.name.clone(),
        help: build_help("profiles", &available, suggestion),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FlagKey, FlagValue, parse::parse_str};

    fn parse(input: &str) -> Config {
        parse_str(input, "test.kdl").expect("invariant: test KDL must parse")
    }

    /// Convenience: extract `(resolved_key, value_string)` pairs and
    /// positionals as `("", value)` so the order can be asserted in
    /// one Vec.
    fn flatten(resolved: &Resolved) -> Vec<(String, String)> {
        resolved
            .args
            .iter()
            .map(|a| match a {
                Argument::Flag { key, value, .. } => {
                    let v = match value {
                        FlagValue::Bool(true) => String::new(),
                        FlagValue::Bool(false) => "<#false>".to_string(),
                        FlagValue::Null => "<#null>".to_string(),
                        FlagValue::Literal(s) => s.clone(),
                    };
                    (key.to_cli_flag(), v)
                }
                Argument::Positional(s) => (String::new(), s.clone()),
            })
            .collect()
    }

    // --- §3.1 / §4 lookup ---

    #[test]
    fn lookup_by_command_name() {
        let cfg = parse("llama-server \"serve\" {\n  host \"0.0.0.0\"\n}\n");
        let r = resolve(&cfg, "llama-server", None, Path::new(".")).unwrap();
        assert_eq!(r.program, "llama-server");
    }

    #[test]
    fn lookup_by_alias() {
        let cfg = parse("llama-server \"serve\" {\n  host \"0.0.0.0\"\n}\n");
        let r = resolve(&cfg, "serve", None, Path::new(".")).unwrap();
        assert_eq!(r.program, "llama-server");
    }

    #[test]
    fn lookup_by_alias_when_name_is_duplicated() {
        let cfg = parse(
            r#"llama-server "serve1" {
                a #true
            }
            llama-server "serve2" {
                b #true
            }"#,
        );
        let r = resolve(&cfg, "serve1", None, Path::new(".")).unwrap();
        assert_eq!(r.program, "llama-server");
        assert_eq!(flatten(&r), vec![("-a".into(), String::new())]);

        let r = resolve(&cfg, "serve2", None, Path::new(".")).unwrap();
        assert_eq!(r.program, "llama-server");
        assert_eq!(flatten(&r), vec![("-b".into(), String::new())]);
    }

    #[test]
    fn bare_name_lookup_of_duplicated_command_is_ambiguous() {
        let cfg = parse(
            r#"llama-server "serve1" { a #true }
            llama-server "serve2" { b #true }"#,
        );
        let err = resolve(&cfg, "llama-server", None, Path::new(".")).unwrap_err();
        let Error::AmbiguousCommand { name, help } = err else {
            panic!("expected AmbiguousCommand");
        };
        assert_eq!(name, "llama-server");
        assert!(help.contains("serve1"));
        assert!(help.contains("serve2"));
    }

    #[test]
    fn ambiguous_help_lists_all_aliases_when_three_duplicates() {
        let cfg = parse(
            r#"foo "a" {}
            foo "b" {}
            foo "c" {}"#,
        );
        let err = resolve(&cfg, "foo", None, Path::new(".")).unwrap_err();
        let Error::AmbiguousCommand { help, .. } = err else {
            panic!("expected AmbiguousCommand");
        };
        assert!(help.contains('a'));
        assert!(help.contains('b'));
        assert!(help.contains('c'));
    }

    #[test]
    fn duplicated_name_excluded_from_did_you_mean() {
        // `foo` is duplicated, so a typo close to `foo` should not
        // suggest it — typing `foo` would have produced an ambiguous
        // error rather than a working invocation.
        let cfg = parse(
            r#"foo "f1" {}
            foo "f2" {}"#,
        );
        let err = resolve(&cfg, "fop", None, Path::new(".")).unwrap_err();
        let Error::UnknownCommand { help, .. } = err else {
            panic!("expected UnknownCommand");
        };
        assert!(
            !help.contains("\"foo\""),
            "did-you-mean should not suggest a duplicated bare name; got: {help}"
        );
    }

    #[test]
    fn unknown_command_with_did_you_mean() {
        let cfg = parse("llama-server {}\ngemma-server {}\n");
        let err = resolve(&cfg, "llama-servr", None, Path::new(".")).unwrap_err();
        let Error::UnknownCommand { name, help } = err else {
            panic!("expected UnknownCommand");
        };
        assert_eq!(name, "llama-servr");
        assert!(help.contains("llama-server"));
        assert!(help.contains("did you mean"));
    }

    #[test]
    fn unknown_profile_with_available_list() {
        let cfg = parse("foo {\n  fast {}\n  slow {}\n}\n");
        let err = resolve(&cfg, "foo", Some("medium"), Path::new(".")).unwrap_err();
        let Error::UnknownProfile {
            profile,
            command,
            help,
        } = err
        else {
            panic!();
        };
        assert_eq!(profile, "medium");
        assert_eq!(command, "foo");
        assert!(help.contains("fast"));
        assert!(help.contains("slow"));
    }

    // --- §5.1 llama-server example ---

    #[test]
    fn spec_5_1_no_profile_yields_defaults_only() {
        let cfg = parse(
            r#"llama-server "serve" {
                host "0.0.0.0"
                port 8090
                c 32768
                flash-attn #true
                qwen-coder {
                    m "/m1"
                    -ngl 999
                }
                llama3 {
                    m "/m2"
                    port 8091
                }
            }"#,
        );
        let r = resolve(&cfg, "serve", None, Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("--host".into(), "0.0.0.0".into()),
                ("--port".into(), "8090".into()),
                ("-c".into(), "32768".into()),
                ("--flash-attn".into(), String::new()),
            ]
        );
    }

    #[test]
    fn spec_5_1_profile_appends_at_profile_position() {
        let cfg = parse(
            r#"llama-server "serve" {
                host "0.0.0.0"
                port 8090
                c 32768
                flash-attn #true
                qwen-coder {
                    m "/m1"
                    -ngl 999
                    -ts "0.5,0.5"
                }
            }"#,
        );
        let r = resolve(&cfg, "serve", Some("qwen-coder"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("--host".into(), "0.0.0.0".into()),
                ("--port".into(), "8090".into()),
                ("-c".into(), "32768".into()),
                ("--flash-attn".into(), String::new()),
                ("-m".into(), "/m1".into()),
                ("-ngl".into(), "999".into()),
                ("-ts".into(), "0.5,0.5".into()),
            ]
        );
    }

    #[test]
    fn spec_5_1_profile_overrides_default_at_default_position() {
        let cfg = parse(
            r#"llama-server "serve" {
                host "0.0.0.0"
                port 8090
                llama3 {
                    m "/m2"
                    port 8091
                }
            }"#,
        );
        let r = resolve(&cfg, "serve", Some("llama3"), Path::new(".")).unwrap();
        // `--port` keeps its default position (between host and the
        // profile's own contributions), but takes the profile's value.
        assert_eq!(
            flatten(&r),
            vec![
                ("--host".into(), "0.0.0.0".into()),
                ("--port".into(), "8091".into()),
                ("-m".into(), "/m2".into()),
            ]
        );
    }

    // --- §2.8.1 first-occurrence positioning ---

    #[test]
    fn first_occurrence_when_default_is_first() {
        let cfg = parse(
            r"some-tool {
                timeout 10
                verbose #true
                fast {
                    timeout 5
                }
            }",
        );
        let r = resolve(&cfg, "some-tool", Some("fast"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("--timeout".into(), "5".into()),
                ("--verbose".into(), String::new()),
            ]
        );
    }

    #[test]
    fn first_occurrence_when_profile_is_first() {
        let cfg = parse(
            r"some-tool {
                fast {
                    timeout 5
                }
                timeout 10
                verbose #true
            }",
        );
        let r = resolve(&cfg, "some-tool", Some("fast"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("--timeout".into(), "5".into()),
                ("--verbose".into(), String::new()),
            ]
        );
    }

    #[test]
    fn single_mode_position_uses_earliest_source_idx_when_profile_precedes_default() {
        // Regression pin for the spec-correct generalisation of
        // §2.8.1: when the selected profile's slot precedes the
        // overriding default in source order, the merged occurrence
        // emits at the profile's slot (earliest source index), not
        // at the default's slot. Pre-inheritance code emitted at
        // the default's slot — this config makes the divergence
        // observable because `verbose` sits between them.
        let cfg = parse(
            r"some-tool {
                fast {
                    timeout 5
                }
                verbose #true
                timeout 10
            }",
        );
        let r = resolve(&cfg, "some-tool", Some("fast"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("--timeout".into(), "5".into()),
                ("--verbose".into(), String::new()),
            ]
        );
    }

    // --- §2.7 / §2.8 profiles as positional slots ---

    #[test]
    fn profile_slot_position_is_preserved() {
        let cfg = parse(
            r#"some-tool {
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
            }"#,
        );
        let r = resolve(&cfg, "some-tool", Some("profile-c"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                (String::new(), "default-positional".into()),
                ("--verbose".into(), String::new()),
                ("--timeout".into(), "90".into()),
            ]
        );
    }

    // --- §2.8.2 cross-type override and suppression ---

    #[test]
    fn quiet_profile_suppresses_everything() {
        let cfg = parse(
            r#"some-tool {
                xxx "test"
                timeout 30
                verbose #true
                quiet {
                    xxx #false
                    timeout #false
                    verbose #false
                }
            }"#,
        );
        let r = resolve(&cfg, "some-tool", Some("quiet"), Path::new(".")).unwrap();
        assert_eq!(flatten(&r), Vec::<(String, String)>::new());
    }

    #[test]
    fn loud_profile_overrides_string_and_number() {
        let cfg = parse(
            r#"some-tool {
                xxx "test"
                timeout 30
                verbose #true
                loud {
                    xxx "verbose-mode"
                    timeout 5
                }
            }"#,
        );
        let r = resolve(&cfg, "some-tool", Some("loud"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("--xxx".into(), "verbose-mode".into()),
                ("--timeout".into(), "5".into()),
                ("--verbose".into(), String::new()),
            ]
        );
    }

    #[test]
    fn flag_form_converts_string_default_to_bare_flag() {
        let cfg = parse(
            r#"some-tool {
                xxx "test"
                timeout 30
                verbose #true
                flag-form { xxx #true }
            }"#,
        );
        let r = resolve(&cfg, "some-tool", Some("flag-form"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("--xxx".into(), String::new()),
                ("--timeout".into(), "30".into()),
                ("--verbose".into(), String::new()),
            ]
        );
    }

    #[test]
    fn no_log_suppresses_string_default_with_false() {
        let cfg = parse(
            r#"some-tool {
                verbose #true
                timeout 30
                log-file "/var/log/some-tool.log"
                no-log { log-file #false }
            }"#,
        );
        let r = resolve(&cfg, "some-tool", Some("no-log"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("--verbose".into(), String::new()),
                ("--timeout".into(), "30".into()),
            ]
        );
    }

    // --- §5.3.1 interleaved positionals ---

    #[test]
    fn ffmpeg_interleaved_positional() {
        let cfg = parse(
            r#"ffmpeg "transcode" {
                h264 {
                    i "input.mp4"
                    -c:v "libx264"
                    "output.mp4"
                }
            }"#,
        );
        let r = resolve(&cfg, "transcode", Some("h264"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("-i".into(), "input.mp4".into()),
                ("-c:v".into(), "libx264".into()),
                (String::new(), "output.mp4".into()),
            ]
        );
    }

    #[test]
    fn git_clone_interleaved() {
        let cfg = parse(
            r#"git {
                clone-myrepo {
                    "clone"
                    "https://github.com/me/repo.git"
                    depth 1
                }
            }"#,
        );
        let r = resolve(&cfg, "git", Some("clone-myrepo"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                (String::new(), "clone".into()),
                (String::new(), "https://github.com/me/repo.git".into()),
                ("--depth".into(), "1".into()),
            ]
        );
    }

    // --- §5.4 boolean vs string distinction ---

    #[test]
    fn enabled_string_vs_bare_flag() {
        let cfg = parse(
            r#"mytool {
                enabled-true {
                    enabled "true"
                }
                enabled-flag {
                    enabled #true
                }
            }"#,
        );
        let r1 = resolve(&cfg, "mytool", Some("enabled-true"), Path::new(".")).unwrap();
        assert_eq!(flatten(&r1), vec![("--enabled".into(), "true".into())]);
        let r2 = resolve(&cfg, "mytool", Some("enabled-flag"), Path::new(".")).unwrap();
        assert_eq!(flatten(&r2), vec![("--enabled".into(), String::new())]);
    }

    // --- did-you-mean nuance ---

    #[test]
    fn did_you_mean_off_by_two_is_caught() {
        let cfg = parse(r"qwen-coder {}");
        let err = resolve(&cfg, "qwen-codr", None, Path::new(".")).unwrap_err();
        let Error::UnknownCommand { help, .. } = err else {
            panic!();
        };
        assert!(help.contains("qwen-coder"));
    }

    #[test]
    fn did_you_mean_returns_none_for_completely_different_input() {
        let cfg = parse(r"qwen-coder {}");
        let err = resolve(&cfg, "totally-unrelated", None, Path::new(".")).unwrap_err();
        let Error::UnknownCommand { help, .. } = err else {
            panic!();
        };
        assert!(!help.contains("did you mean"));
    }

    #[test]
    fn flag_key_field_used() {
        // Sanity-check that resolve preserves FlagKey distinctions.
        let cfg = parse("foo {\n  -ngl 999\n  verbose #true\n}\n");
        let r = resolve(&cfg, "foo", None, Path::new(".")).unwrap();
        let Argument::Flag { key, .. } = &r.args[0] else {
            panic!();
        };
        assert!(matches!(key, FlagKey::Verbatim(s) if s == "-ngl"));
    }

    // --- §2.8 repeat-mode (multi-occurrence) merge ---

    #[test]
    fn defaults_only_repeated_keys_emit_in_order() {
        // gcc-style: two unmarked default occurrences resolve to
        // repeat mode (|D_unmarked| > 1).
        let cfg = parse(
            r#"gcc {
                I "/usr/include"
                I "/opt/include"
            }"#,
        );
        let r = resolve(&cfg, "gcc", None, Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("-I".into(), "/usr/include".into()),
                ("-I".into(), "/opt/include".into()),
            ]
        );
    }

    #[test]
    fn profile_only_repeated_keys_emit_in_order() {
        let cfg = parse(
            r#"curl {
                with-headers {
                    header "X-A: 1"
                    header "X-B: 2"
                }
            }"#,
        );
        let r = resolve(&cfg, "curl", Some("with-headers"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("--header".into(), "X-A: 1".into()),
                ("--header".into(), "X-B: 2".into()),
            ]
        );
    }

    #[test]
    fn repeat_mode_when_default_has_two_and_profile_adds_one() {
        let cfg = parse(
            r#"gcc {
                I "/usr/include"
                I "/opt/include"
                project-a {
                    I "/proj/a/include"
                }
            }"#,
        );
        let r = resolve(&cfg, "gcc", Some("project-a"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("-I".into(), "/usr/include".into()),
                ("-I".into(), "/opt/include".into()),
                ("-I".into(), "/proj/a/include".into()),
            ]
        );
    }

    #[test]
    fn repeat_mode_when_default_has_one_and_profile_has_two() {
        // |D_unmarked|=1, |P_unmarked|=2 → repeat mode → all three
        // emit. The default does NOT get overridden in this shape.
        let cfg = parse(
            r#"gcc {
                I "/usr/include"
                add-two {
                    I "/a"
                    I "/b"
                }
            }"#,
        );
        let r = resolve(&cfg, "gcc", Some("add-two"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("-I".into(), "/usr/include".into()),
                ("-I".into(), "/a".into()),
                ("-I".into(), "/b".into()),
            ]
        );
    }

    #[test]
    fn count_flag_pattern_v_three_times() {
        // Three occurrences of `v #true` resolve to `-v -v -v`.
        let cfg = parse(
            r"some-tool {
                v #true
                v #true
                v #true
            }",
        );
        let r = resolve(&cfg, "some-tool", None, Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("-v".into(), String::new()),
                ("-v".into(), String::new()),
                ("-v".into(), String::new()),
            ]
        );
    }

    // --- §2.8 markerless `#false` clear ---

    #[test]
    fn profile_false_clears_multi_default_list() {
        let cfg = parse(
            r#"gcc {
                I "/a"
                I "/b"
                bare {
                    I #false
                }
            }"#,
        );
        let r = resolve(&cfg, "gcc", Some("bare"), Path::new(".")).unwrap();
        assert_eq!(flatten(&r), Vec::<(String, String)>::new());
    }

    #[test]
    fn profile_false_then_value_clears_then_adds() {
        // The profile clears defaults' I list, then adds its own.
        let cfg = parse(
            r#"gcc {
                I "/a"
                I "/b"
                custom {
                    I #false
                    I "/mine"
                }
            }"#,
        );
        let r = resolve(&cfg, "gcc", Some("custom"), Path::new(".")).unwrap();
        // After clear+add, we're left with one profile occurrence —
        // single mode emits it at the profile's position.
        assert_eq!(flatten(&r), vec![("-I".into(), "/mine".into())]);
    }

    #[test]
    fn default_false_in_middle_of_repeats_drops_only_itself() {
        let cfg = parse(
            r#"gcc {
                I "/a"
                I #false
                I "/b"
            }"#,
        );
        let r = resolve(&cfg, "gcc", None, Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![("-I".into(), "/a".into()), ("-I".into(), "/b".into()),]
        );
    }

    // --- §2.5 / §2.8 explicit append marker ---

    #[test]
    fn marked_profile_adds_to_single_default() {
        // The blind-spot case for the markerless rule: `+` lets the
        // profile add an occurrence without overriding the default.
        let cfg = parse(
            r#"gcc {
                I "/usr/include"
                proj-extras {
                    +I "/proj/include"
                }
            }"#,
        );
        let r = resolve(&cfg, "gcc", Some("proj-extras"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("-I".into(), "/usr/include".into()),
                ("-I".into(), "/proj/include".into()),
            ]
        );
    }

    #[test]
    fn unmarked_profile_overrides_single_default_in_v1_mode() {
        // Same shape as above but without the `+`: single+single
        // single-mode → v1 override at default's position.
        let cfg = parse(
            r#"gcc {
                I "/usr/include"
                proj-replace {
                    I "/proj/include"
                }
            }"#,
        );
        let r = resolve(&cfg, "gcc", Some("proj-replace"), Path::new(".")).unwrap();
        assert_eq!(flatten(&r), vec![("-I".into(), "/proj/include".into())]);
    }

    #[test]
    fn unmarked_overrides_default_marked_emits_separately() {
        // Profile has one unmarked + one marked. Unmarked single-mode
        // overrides the default at default's position; marked emits
        // at its own position.
        let cfg = parse(
            r#"gcc {
                I "/usr/include"
                mixed {
                    I "/replace"
                    +I "/extra"
                }
            }"#,
        );
        let r = resolve(&cfg, "gcc", Some("mixed"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("-I".into(), "/replace".into()),
                ("-I".into(), "/extra".into()),
            ]
        );
    }

    #[test]
    fn marked_default_emits_then_unmarked_single_mode() {
        // Default has a marked + an unmarked. Profile has one
        // unmarked. The marked default always emits; the unmarked
        // default + unmarked profile resolve in single mode.
        let cfg = parse(
            r#"gcc {
                +I "/always"
                I "/dflt"
                proj {
                    I "/proj"
                }
            }"#,
        );
        let r = resolve(&cfg, "gcc", Some("proj"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("-I".into(), "/always".into()),
                ("-I".into(), "/proj".into()),
            ]
        );
    }

    #[test]
    fn profile_false_clears_marked_default_too() {
        // Profile-side `#false` clears every default occurrence of
        // the key, regardless of marker.
        let cfg = parse(
            r#"gcc {
                +I "/always"
                I "/dflt"
                bare {
                    I #false
                }
            }"#,
        );
        let r = resolve(&cfg, "gcc", Some("bare"), Path::new(".")).unwrap();
        assert_eq!(flatten(&r), Vec::<(String, String)>::new());
    }

    #[test]
    fn marked_profile_with_no_default() {
        // `+` on a profile flag with no matching default is harmless:
        // it's the only entry, emits at its own position.
        let cfg = parse(
            r#"foo {
                proj {
                    +I "/proj"
                }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("proj"), Path::new(".")).unwrap();
        assert_eq!(flatten(&r), vec![("-I".into(), "/proj".into())]);
    }

    #[test]
    fn two_marked_entries_in_one_profile() {
        // Two `+`-marked entries for the same key in one profile —
        // both should emit at their own positions, since marker
        // skips collapse.
        let cfg = parse(
            r#"foo {
                proj {
                    +I "/a"
                    +I "/b"
                }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("proj"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![("-I".into(), "/a".into()), ("-I".into(), "/b".into()),]
        );
    }

    #[test]
    fn marked_default_with_no_profile_selected() {
        // `+` in defaults with no profile selected is harmless: only
        // entry, emits at its own position.
        let cfg = parse(r#"foo { +I "/dflt" }"#);
        let r = resolve(&cfg, "foo", None, Path::new(".")).unwrap();
        assert_eq!(flatten(&r), vec![("-I".into(), "/dflt".into())]);
    }

    #[test]
    fn marked_boolean_flag_emits() {
        // `+v #true` is a marked boolean: marker forces own-position
        // emit, value `#true` emits as bare flag (no value).
        let cfg = parse(
            r"foo {
                v #true
                more {
                    +v #true
                }
            }",
        );
        let r = resolve(&cfg, "foo", Some("more"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![("-v".into(), String::new()), ("-v".into(), String::new()),]
        );
    }

    #[test]
    fn resolved_form_collision_keys_marked_and_unmarked() {
        // `+host` and unmarked `--host` both resolve to `--host`,
        // so they share the same per-key plan. Marked emits at its
        // own position; unmarked single-mode emits at its position
        // with its own value (no profile to override with).
        let cfg = parse(
            r#"foo {
                +host "a"
                --host "b"
            }"#,
        );
        let r = resolve(&cfg, "foo", None, Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![("--host".into(), "a".into()), ("--host".into(), "b".into()),]
        );
    }

    // --- §2.10 / §2.11 env-var resolution ---

    #[test]
    fn env_defaults_only_no_profile() {
        let cfg = parse(
            r#"foo {
                (env)A "1"
                (env)B "2"
            }"#,
        );
        let r = resolve(&cfg, "foo", None, Path::new(".")).unwrap();
        assert_eq!(
            r.env,
            vec![
                EnvOp::Set {
                    name: "A".into(),
                    value: "1".into()
                },
                EnvOp::Set {
                    name: "B".into(),
                    value: "2".into()
                },
            ]
        );
    }

    #[test]
    fn env_profile_only_when_no_defaults() {
        let cfg = parse(
            r#"foo {
                fast {
                    (env)X "y"
                }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("fast"), Path::new(".")).unwrap();
        assert_eq!(
            r.env,
            vec![EnvOp::Set {
                name: "X".into(),
                value: "y".into()
            }]
        );
    }

    #[test]
    fn env_profile_overrides_default() {
        let cfg = parse(
            r#"foo {
                (env)A "default"
                fast {
                    (env)A "override"
                }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("fast"), Path::new(".")).unwrap();
        assert_eq!(
            r.env,
            vec![EnvOp::Set {
                name: "A".into(),
                value: "override".into()
            }]
        );
    }

    #[test]
    fn env_profile_unset_clears_default() {
        let cfg = parse(
            r#"foo {
                (env)A "default"
                clear {
                    (env)A #false
                }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("clear"), Path::new(".")).unwrap();
        assert_eq!(r.env, vec![EnvOp::Unset { name: "A".into() }]);
    }

    #[test]
    fn env_default_unset_passes_through_when_no_profile_override() {
        let cfg = parse(
            r"foo {
                (env)PATH #false
            }",
        );
        let r = resolve(&cfg, "foo", None, Path::new(".")).unwrap();
        assert_eq!(
            r.env,
            vec![EnvOp::Unset {
                name: "PATH".into()
            }]
        );
    }

    #[test]
    fn env_profile_set_overrides_default_unset() {
        // Per §2.11 precedence: profile-side `Set` wins over
        // default-side `Unset`. The default would otherwise call
        // env_remove; the profile re-introduces the variable.
        let cfg = parse(
            r#"foo {
                (env)A #false
                fast {
                    (env)A "from-profile"
                }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("fast"), Path::new(".")).unwrap();
        assert_eq!(
            r.env,
            vec![EnvOp::Set {
                name: "A".into(),
                value: "from-profile".into()
            }]
        );
    }

    #[test]
    fn env_empty_string_value_round_trips() {
        // `""` is a meaningful POSIX env value (set, but empty),
        // distinct from `#false` (unset). Pin that we don't drop it.
        let cfg = parse(r#"foo { (env)A "" }"#);
        let r = resolve(&cfg, "foo", None, Path::new(".")).unwrap();
        assert_eq!(
            r.env,
            vec![EnvOp::Set {
                name: "A".into(),
                value: String::new()
            }]
        );
    }

    #[test]
    fn env_first_occurrence_ordering_default_first() {
        // Defaults define A then B; profile overrides B and adds C.
        // Expected order: A, B, C — defaults' walk order then profile
        // additions.
        let cfg = parse(
            r#"foo {
                (env)A "1"
                (env)B "2"
                fast {
                    (env)B "overridden"
                    (env)C "3"
                }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("fast"), Path::new(".")).unwrap();
        assert_eq!(
            r.env,
            vec![
                EnvOp::Set {
                    name: "A".into(),
                    value: "1".into()
                },
                EnvOp::Set {
                    name: "B".into(),
                    value: "overridden".into()
                },
                EnvOp::Set {
                    name: "C".into(),
                    value: "3".into()
                },
            ]
        );
    }

    #[test]
    fn env_does_not_appear_in_args() {
        // §2.10: env decls do not appear on the resolved argv.
        let cfg = parse(
            r#"foo {
                host "0.0.0.0"
                (env)OLLAMA_HOST "1"
            }"#,
        );
        let r = resolve(&cfg, "foo", None, Path::new(".")).unwrap();
        assert_eq!(flatten(&r), vec![("--host".into(), "0.0.0.0".into())]);
        assert_eq!(r.env.len(), 1);
    }

    #[test]
    fn count_flag_clear_then_replace() {
        // Profile clears the default count and sets a different one.
        // Profile `#false` wipes defaults; the two surviving
        // unmarked profile entries trigger repeat mode.
        let cfg = parse(
            r"foo {
                v #true
                v #true
                v #true
                medium {
                    v #false
                    v #true
                    v #true
                }
            }",
        );
        let r = resolve(&cfg, "foo", Some("medium"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![("-v".into(), String::new()), ("-v".into(), String::new()),]
        );
    }

    // --- §2.8.5 profile inheritance: N-tier cascade ---

    #[test]
    fn inheritance_chain_helper_returns_root_to_leaf() {
        let cfg = parse(
            r#"foo {
                grand {}
                parent extends="grand" {}
                child extends="parent" {}
            }"#,
        );
        let chain = inheritance_chain(&cfg.commands[0], "child");
        assert_eq!(chain, vec!["grand", "parent", "child"]);
    }

    #[test]
    fn inheritance_two_level_child_overrides_parent_flag() {
        let cfg = parse(
            r#"foo {
                qwen-coder {
                    m "/p1"
                    -ngl 999
                }
                qwen-coder-large extends="qwen-coder" {
                    m "/p2"
                }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("qwen-coder-large"), Path::new(".")).unwrap();
        // `m`: parent (tier 1, idx 0) + child (tier 2, idx 2). Each
        // tier has ≤ 1 → single mode. Highest-tier value (child's
        // `/p2`) wins; earliest idx is the parent's slot (0).
        // `-ngl`: only tier 1 (idx 1). Single mode at idx 1.
        assert_eq!(
            flatten(&r),
            vec![("-m".into(), "/p2".into()), ("-ngl".into(), "999".into()),]
        );
    }

    #[test]
    fn inheritance_three_level_leaf_wins() {
        let cfg = parse(
            r#"foo {
                grand { x "from-grand" }
                parent extends="grand" { x "from-parent" }
                child extends="parent" { x "from-child" }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("child"), Path::new(".")).unwrap();
        assert_eq!(flatten(&r), vec![("-x".into(), "from-child".into())]);
    }

    #[test]
    fn inheritance_selecting_middle_starts_shorter_chain() {
        // Selecting `parent` activates `grand` + `parent` but not
        // `child`. The leaf-side value comes from `parent`.
        let cfg = parse(
            r#"foo {
                grand { x "from-grand" }
                parent extends="grand" { x "from-parent" }
                child extends="parent" { x "from-child" }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("parent"), Path::new(".")).unwrap();
        assert_eq!(flatten(&r), vec![("-x".into(), "from-parent".into())]);
    }

    #[test]
    fn inheritance_unselected_sibling_does_not_activate() {
        // `sibling` extends `parent` too but isn't selected — its
        // body must not appear in the resolved output.
        let cfg = parse(
            r#"foo {
                parent { x "from-parent" }
                child extends="parent" { y "from-child" }
                sibling extends="parent" { z "from-sibling" }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("child"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("-x".into(), "from-parent".into()),
                ("-y".into(), "from-child".into()),
            ]
        );
    }

    #[test]
    fn inheritance_forward_declaration_emits_at_each_slot() {
        // Child declared before parent in source order. Each profile
        // still emits at its own slot.
        let cfg = parse(
            r#"foo {
                child extends="parent" { y "from-child" }
                parent { x "from-parent" }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("child"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("-y".into(), "from-child".into()),
                ("-x".into(), "from-parent".into()),
            ]
        );
    }

    #[test]
    fn inheritance_false_at_middle_tier_wipes_lower_only() {
        let cfg = parse(
            r#"foo {
                x "default"
                grand { x "from-grand" }
                parent extends="grand" { x #false }
                child extends="parent" { x "from-child" }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("child"), Path::new(".")).unwrap();
        // For `x`: tier-2 `#false` clears tier 0 + tier 1. Tier 3
        // (`from-child`) survives; single mode emits it.
        assert_eq!(flatten(&r), vec![("-x".into(), "from-child".into())]);
    }

    #[test]
    fn inheritance_false_at_leaf_clears_marker_at_ancestor() {
        // Leaf `#false` wipes every lower tier, including a `+`
        // ancestor entry, because the marker rule applies after
        // tier-based suppression.
        let cfg = parse(
            r#"foo {
                parent {
                    +I "/extra"
                }
                child extends="parent" {
                    I #false
                }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("child"), Path::new(".")).unwrap();
        assert_eq!(flatten(&r), Vec::<(String, String)>::new());
    }

    #[test]
    fn inheritance_marker_at_ancestor_survives_with_leaf_value() {
        let cfg = parse(
            r#"foo {
                parent {
                    +I "/extra"
                }
                child extends="parent" {
                    I "/main"
                }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("child"), Path::new(".")).unwrap();
        // Parent `+I /extra` emits at its own slot (idx 0); child
        // unmarked `I /main` collapses (each tier has 1 unmarked
        // ⇒ single mode), value from highest tier, position at
        // earliest idx (child's slot, idx 1).
        assert_eq!(
            flatten(&r),
            vec![
                ("-I".into(), "/extra".into()),
                ("-I".into(), "/main".into()),
            ]
        );
    }

    #[test]
    fn inheritance_repeat_mode_from_ancestor_and_leaf_combined() {
        let cfg = parse(
            r#"gcc {
                parent { I "/a" }
                child extends="parent" {
                    I "/b"
                    I "/c"
                }
            }"#,
        );
        let r = resolve(&cfg, "gcc", Some("child"), Path::new(".")).unwrap();
        // Tier 1 has 1 unmarked `I`, tier 2 has 2 → not all ≤ 1 →
        // repeat mode. Every unmarked emits at its own position.
        assert_eq!(
            flatten(&r),
            vec![
                ("-I".into(), "/a".into()),
                ("-I".into(), "/b".into()),
                ("-I".into(), "/c".into()),
            ]
        );
    }

    #[test]
    fn inheritance_repeat_mode_includes_defaults_too() {
        let cfg = parse(
            r#"gcc {
                I "/sys1"
                I "/sys2"
                parent { I "/p" }
                child extends="parent" { I "/c" }
            }"#,
        );
        let r = resolve(&cfg, "gcc", Some("child"), Path::new(".")).unwrap();
        // Tier 0 has 2 unmarked → repeat mode triggered. All four
        // unmarked entries emit at their own position.
        assert_eq!(
            flatten(&r),
            vec![
                ("-I".into(), "/sys1".into()),
                ("-I".into(), "/sys2".into()),
                ("-I".into(), "/p".into()),
                ("-I".into(), "/c".into()),
            ]
        );
    }

    #[test]
    fn inheritance_defaults_and_chain_all_cascade() {
        // Three-tier cascade exercising defaults + parent + child.
        // `host` is set in defaults and parent → child overrides
        // nothing → highest-tier value wins (parent's).
        // `port` is set in defaults and child → child wins.
        // `m` is only in child → only tier-3 source.
        let cfg = parse(
            r#"foo {
                host "0.0.0.0"
                port 8090
                parent {
                    host "parent-host"
                    -ngl 999
                }
                child extends="parent" {
                    port 8091
                    m "/p"
                }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("child"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("--host".into(), "parent-host".into()),
                ("--port".into(), "8091".into()),
                ("-ngl".into(), "999".into()),
                ("-m".into(), "/p".into()),
            ]
        );
    }

    #[test]
    fn inheritance_env_leaf_resets_ancestor_unset() {
        let cfg = parse(
            r#"foo {
                parent { (env)A #false }
                child extends="parent" { (env)A "from-leaf" }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("child"), Path::new(".")).unwrap();
        assert_eq!(
            r.env,
            vec![EnvOp::Set {
                name: "A".into(),
                value: "from-leaf".into(),
            }]
        );
    }

    #[test]
    fn inheritance_env_leaf_unsets_ancestor_value() {
        let cfg = parse(
            r#"foo {
                parent { (env)A "from-parent" }
                child extends="parent" { (env)A #false }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("child"), Path::new(".")).unwrap();
        assert_eq!(r.env, vec![EnvOp::Unset { name: "A".into() }]);
    }

    #[test]
    fn inheritance_env_ancestor_only_var_passes_through() {
        let cfg = parse(
            r#"foo {
                (env)A "default"
                parent { (env)B "from-parent" }
                child extends="parent" {}
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("child"), Path::new(".")).unwrap();
        assert_eq!(
            r.env,
            vec![
                EnvOp::Set {
                    name: "A".into(),
                    value: "default".into(),
                },
                EnvOp::Set {
                    name: "B".into(),
                    value: "from-parent".into(),
                },
            ]
        );
    }

    #[test]
    fn inheritance_env_three_tier_cascade() {
        let cfg = parse(
            r#"foo {
                (env)A "default"
                (env)B "default"
                parent {
                    (env)A "from-parent"
                    (env)C "from-parent"
                }
                child extends="parent" {
                    (env)A "from-child"
                }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("child"), Path::new(".")).unwrap();
        // A: defaults → parent → child. Highest tier (child) wins.
        // B: only in defaults.
        // C: only in parent.
        // Order is first-occurrence across ascending walk: A (idx 0
        // in defaults), B (idx 1 in defaults), C (parent's first
        // appearance).
        assert_eq!(
            r.env,
            vec![
                EnvOp::Set {
                    name: "A".into(),
                    value: "from-child".into(),
                },
                EnvOp::Set {
                    name: "B".into(),
                    value: "default".into(),
                },
                EnvOp::Set {
                    name: "C".into(),
                    value: "from-parent".into(),
                },
            ]
        );
    }

    #[test]
    fn inheritance_leaf_false_clears_every_lower_tier() {
        // The leaf's `#false` sets max_false_tier to its tier (3),
        // which must drop the tier-0 default value AND the tier-1
        // parent value. Pins that the cascade applies to *all*
        // lower tiers, not just the immediate parent.
        let cfg = parse(
            r#"foo {
                x "from-default"
                parent { x "from-parent" }
                grandchild extends="parent" {}
                leaf extends="grandchild" { x #false }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("leaf"), Path::new(".")).unwrap();
        assert_eq!(flatten(&r), Vec::<(String, String)>::new());
    }

    #[test]
    fn inheritance_marked_false_clears_lower_tiers_too() {
        // A `+`-prefixed `#false` at a profile tier is still a
        // `#false`: it raises max_false_tier and wipes lower tiers.
        // Pins that the marker does not opt the entry out of
        // suppression's tier-drop.
        let cfg = parse(
            r#"foo {
                x "from-default"
                parent { x "from-parent" }
                child extends="parent" { +x #false }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("child"), Path::new(".")).unwrap();
        assert_eq!(flatten(&r), Vec::<(String, String)>::new());
    }

    #[test]
    fn inheritance_unselected_sibling_interleaved_does_not_leak() {
        // An unselected sibling sitting between two activated chain
        // profiles in source order must contribute nothing. Pins
        // that the `tier_of` filter rejects non-chain profiles
        // even when they share a key with the chain.
        let cfg = parse(
            r#"foo {
                parent { x "from-parent" }
                other {
                    x "from-other"
                    y "leaked"
                }
                child extends="parent" { z "from-child" }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("child"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![
                ("-x".into(), "from-parent".into()),
                ("-z".into(), "from-child".into()),
            ]
        );
    }

    // --- §2.4.3 `#null` placeholder ---

    #[test]
    fn null_default_alone_emits_nothing() {
        // `a #null` declares the flag at idx 0 but contributes no
        // value. With no profile filling it in, the flag does not
        // emit.
        let cfg = parse(
            r"foo {
                a #null
                b 123
            }",
        );
        let r = resolve(&cfg, "foo", None, Path::new(".")).unwrap();
        assert_eq!(flatten(&r), vec![("-b".into(), "123".into())]);
    }

    #[test]
    fn null_default_filled_by_profile_emits_at_default_position() {
        // The canonical placeholder pattern: declare the flag at the
        // command level with `#null`, let a profile supply the value,
        // emit at the default's slot.
        let cfg = parse(
            r#"foo {
                a #null
                b 123
                p { a "x" }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("p"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![("-a".into(), "x".into()), ("-b".into(), "123".into())]
        );
    }

    #[test]
    fn null_default_after_value_default_keeps_first_position() {
        // The earlier of two same-key default occurrences wins the
        // position. `a "x"; a #null` puts the position at idx 0
        // regardless of which side later supplies the value.
        let cfg = parse(
            r#"foo {
                a "x"
                a #null
                p { a "y" }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("p"), Path::new(".")).unwrap();
        // Tier 0 has one survivor (`"x"` at idx 0). Tier 1 has one
        // survivor (`"y"` at idx 2). The `#null` at idx 1 is a ghost.
        // Single mode: earliest survivor idx is 0, highest-tier
        // value is `"y"`. Emit at idx 0 with `"y"`.
        assert_eq!(flatten(&r), vec![("-a".into(), "y".into())]);
    }

    #[test]
    fn null_in_profile_alone_emits_nothing() {
        // A profile-only `#null` has no value to emit. The flag
        // never appears.
        let cfg = parse(
            r"foo {
                p { a #null }
            }",
        );
        let r = resolve(&cfg, "foo", Some("p"), Path::new(".")).unwrap();
        assert_eq!(flatten(&r), Vec::<(String, String)>::new());
    }

    #[test]
    fn value_default_with_null_profile_keeps_default_value() {
        // Profile-side `#null` is a no-op for value selection; it
        // doesn't override or suppress the default.
        let cfg = parse(
            r#"foo {
                a "x"
                p { a #null }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("p"), Path::new(".")).unwrap();
        assert_eq!(flatten(&r), vec![("-a".into(), "x".into())]);
    }

    #[test]
    fn null_does_not_trigger_repeat_mode() {
        // Two unmarked defaults plus a profile `#null` is still
        // mode-selected on the two defaults: repeat mode emits each
        // default occurrence, the null contributes nothing.
        let cfg = parse(
            r#"foo {
                a "x"
                a "y"
                p { a #null }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("p"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![("-a".into(), "x".into()), ("-a".into(), "y".into())]
        );
    }

    #[test]
    fn null_does_not_clear_other_defaults() {
        // Unlike profile-side `#false`, profile-side `#null` does
        // not clear lower-tier entries of the same key.
        let cfg = parse(
            r#"foo {
                a "x"
                a "y"
                bare { a #null }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("bare"), Path::new(".")).unwrap();
        assert_eq!(
            flatten(&r),
            vec![("-a".into(), "x".into()), ("-a".into(), "y".into())]
        );
    }

    #[test]
    fn null_inheritance_default_ghost_with_chain_value_at_default_position() {
        // `a #null` in defaults reserves idx 0; parent supplies the
        // value at tier 1; child inherits unchanged. Single mode
        // emits at idx 0 (earliest ghost-or-survivor) with parent's
        // value.
        let cfg = parse(
            r#"foo {
                a #null
                parent { a "p" }
                child extends="parent" {}
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("child"), Path::new(".")).unwrap();
        assert_eq!(flatten(&r), vec![("-a".into(), "p".into())]);
    }

    #[test]
    fn null_inheritance_leaf_ghost_with_default_value_keeps_default() {
        // A leaf `#null` is a no-op; the highest non-null tier
        // (defaults here) supplies the value.
        let cfg = parse(
            r#"foo {
                a "x"
                parent {}
                child extends="parent" { a #null }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("child"), Path::new(".")).unwrap();
        assert_eq!(flatten(&r), vec![("-a".into(), "x".into())]);
    }

    #[test]
    fn null_ghost_does_not_move_marked_entry_position() {
        // A `+`-marked profile entry always emits at its own
        // source position. A `#null` ghost in defaults must not
        // pull a marked entry into the ghost's slot — markers are
        // outside the single-mode collapse.
        let cfg = parse(
            r#"foo {
                a #null
                proj { +a "/extra" }
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("proj"), Path::new(".")).unwrap();
        // The `+`-marked entry is at candidate idx 1 (after the
        // tier-0 ghost at idx 0). It emits at idx 1, not idx 0.
        // Output order from a single-flag config is just `-a /extra`.
        assert_eq!(flatten(&r), vec![("-a".into(), "/extra".into())]);
    }

    #[test]
    fn null_in_unselected_sibling_profile_does_not_leak() {
        // A `#null` in a profile that isn't selected contributes
        // nothing to the candidate list; only the chain's profiles
        // (and defaults) are walked.
        let cfg = parse(
            r#"foo {
                a "x"
                other { a #null }
                p {}
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("p"), Path::new(".")).unwrap();
        assert_eq!(flatten(&r), vec![("-a".into(), "x".into())]);
    }

    #[test]
    fn null_with_profile_false_at_higher_tier_clears_ghost() {
        // Profile-side `#false` triggers the T-cascade: every entry
        // at tier < T is dropped, including `#null` ghosts. With
        // nothing surviving above T (the #false itself doesn't
        // survive), no emission.
        let cfg = parse(
            r"foo {
                a #null
                bare { a #false }
            }",
        );
        let r = resolve(&cfg, "foo", Some("bare"), Path::new(".")).unwrap();
        assert_eq!(flatten(&r), Vec::<(String, String)>::new());
    }

    // --- §2.12 effective cwd ---

    #[test]
    fn cwd_none_without_property() {
        let cfg = parse(r#"foo { host "0.0.0.0" }"#);
        let r = resolve(&cfg, "foo", None, Path::new("/anchor")).unwrap();
        assert!(r.cwd.is_none());
    }

    #[test]
    fn cwd_command_absolute_used_as_is() {
        let cfg = parse(r#"foo cwd="/abs" {}"#);
        let r = resolve(&cfg, "foo", None, Path::new("/anchor")).unwrap();
        assert_eq!(r.cwd, Some(PathBuf::from("/abs")));
    }

    #[test]
    fn cwd_command_relative_anchored_to_config_dir() {
        let cfg = parse(r#"foo cwd="src" {}"#);
        let r = resolve(&cfg, "foo", None, Path::new("/proj")).unwrap();
        assert_eq!(r.cwd, Some(PathBuf::from("/proj/src")));
    }

    #[test]
    fn cwd_dot_resolves_to_config_dir() {
        let cfg = parse(r#"foo cwd="." {}"#);
        let r = resolve(&cfg, "foo", None, Path::new("/proj")).unwrap();
        assert_eq!(r.cwd, Some(PathBuf::from("/proj/.")));
    }

    #[test]
    fn cwd_profile_overrides_command() {
        let cfg = parse(
            r#"foo cwd="/cmd" {
                fast cwd="/profile" {}
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("fast"), Path::new("/anchor")).unwrap();
        assert_eq!(r.cwd, Some(PathBuf::from("/profile")));
    }

    #[test]
    fn cwd_profile_without_cwd_inherits_command() {
        let cfg = parse(
            r#"foo cwd="/cmd" {
                fast {}
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("fast"), Path::new("/anchor")).unwrap();
        assert_eq!(r.cwd, Some(PathBuf::from("/cmd")));
    }

    #[test]
    fn cwd_extends_chain_leaf_wins() {
        let cfg = parse(
            r#"foo {
                grand cwd="/grand" {}
                parent extends="grand" cwd="/parent" {}
                child extends="parent" cwd="/child" {}
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("child"), Path::new("/anchor")).unwrap();
        assert_eq!(r.cwd, Some(PathBuf::from("/child")));
    }

    #[test]
    fn cwd_extends_chain_inherits_from_ancestor_when_leaf_missing() {
        // The leaf has no cwd; walking the chain leaf-to-root picks
        // up the parent's cwd before falling all the way back to the
        // command's.
        let cfg = parse(
            r#"foo cwd="/cmd" {
                parent cwd="/parent" {}
                child extends="parent" {}
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("child"), Path::new("/anchor")).unwrap();
        assert_eq!(r.cwd, Some(PathBuf::from("/parent")));
    }

    #[test]
    fn cwd_relative_on_profile_uses_same_anchor() {
        // The anchor for relative paths is always the config-file
        // directory, regardless of which tier supplied the value.
        let cfg = parse(
            r#"foo {
                fast cwd="rel" {}
            }"#,
        );
        let r = resolve(&cfg, "foo", Some("fast"), Path::new("/anchor")).unwrap();
        assert_eq!(r.cwd, Some(PathBuf::from("/anchor/rel")));
    }

    #[test]
    fn cwd_no_profile_selected_uses_command_value() {
        let cfg = parse(r#"foo cwd="here" { fast cwd="there" {} }"#);
        let r = resolve(&cfg, "foo", None, Path::new("/proj")).unwrap();
        assert_eq!(r.cwd, Some(PathBuf::from("/proj/here")));
    }

    // --- ResolutionTrace ---

    #[test]
    fn trace_defaults_only_records_sole_contributors() {
        let cfg = parse(
            r#"foo {
                host "0.0.0.0"
                port 8090
            }"#,
        );
        let (resolved, trace) = resolve_with_trace(&cfg, "foo", None, Path::new(".")).unwrap();
        assert_eq!(resolved.args.len(), 2);
        assert_eq!(trace.segments.len(), 2);
        for seg in &trace.segments {
            assert!(
                seg.mode_summary.is_none(),
                "single contributor → no summary"
            );
            assert_eq!(seg.contributors.len(), 1);
            let c = &seg.contributors[0];
            assert!(matches!(c.role, ContributorRole::Sole));
            assert_eq!(c.tier, 0);
            assert_eq!(c.tier_label, "defaults");
            assert!(!c.inherited);
            assert!(seg.dropped.is_empty());
        }
        assert!(trace.suppressed.is_empty());
        assert!(trace.env.is_empty());
        assert!(matches!(trace.cwd, CwdTrace::Inherited));
    }

    #[test]
    fn trace_single_mode_override_records_position_and_value_winners() {
        let cfg = parse(
            r#"foo {
                host "default"
                p { host "override" }
            }"#,
        );
        let (_, trace) = resolve_with_trace(&cfg, "foo", Some("p"), Path::new(".")).unwrap();
        assert_eq!(trace.segments.len(), 1);
        let seg = &trace.segments[0];
        assert!(seg.mode_summary.is_some(), "merge → mode_summary set");
        assert_eq!(seg.contributors.len(), 2);
        let pos = seg
            .contributors
            .iter()
            .find(|c| matches!(c.role, ContributorRole::PositionOnly));
        let val = seg
            .contributors
            .iter()
            .find(|c| matches!(c.role, ContributorRole::ValueOnly));
        let pos = pos.expect("expected a position-only contributor");
        let val = val.expect("expected a value-only contributor");
        assert_eq!(pos.tier_label, "defaults");
        assert_eq!(val.tier_label, "p");
        // Two-tier override: variant C suppresses any "dropped" line.
        assert!(seg.dropped.is_empty(), "two-tier override hides drops");
    }

    #[test]
    fn trace_inheritance_marks_ancestors_as_inherited() {
        let cfg = parse(
            r#"foo {
                parent { host "p-host" }
                child extends="parent" { port 8091 }
            }"#,
        );
        let (_, trace) = resolve_with_trace(&cfg, "foo", Some("child"), Path::new(".")).unwrap();
        assert_eq!(trace.chain, vec!["parent", "child"]);
        // Two segments: parent's --host (inherited), child's --port (not).
        let host_seg = trace
            .segments
            .iter()
            .find(|s| s.contributors.iter().any(|c| c.tier_label == "parent"))
            .expect("expected --host segment with parent contributor");
        let parent_c = host_seg
            .contributors
            .iter()
            .find(|c| c.tier_label == "parent")
            .unwrap();
        assert!(parent_c.inherited, "parent is an ancestor → inherited");

        let port_seg = trace
            .segments
            .iter()
            .find(|s| s.contributors.iter().any(|c| c.tier_label == "child"))
            .expect("expected --port segment with child contributor");
        let child_c = port_seg
            .contributors
            .iter()
            .find(|c| c.tier_label == "child")
            .unwrap();
        assert!(!child_c.inherited, "child is the leaf → not inherited");
    }

    #[test]
    fn trace_suppression_no_segment_records_in_suppressed() {
        let cfg = parse(
            r#"foo {
                host "x"
                quiet { host #false }
            }"#,
        );
        let (resolved, trace) =
            resolve_with_trace(&cfg, "foo", Some("quiet"), Path::new(".")).unwrap();
        assert!(resolved.args.is_empty(), "host wiped → no argv");
        assert!(trace.segments.is_empty());
        assert_eq!(trace.suppressed.len(), 1);
        let s = &trace.suppressed[0];
        assert_eq!(s.key, "--host");
        // One cleared default + one suppressor (#false) entry.
        assert_eq!(s.cleared.len(), 2);
        let suppressor = s
            .cleared
            .iter()
            .find(|d| matches!(d.reason, DroppedReason::SelfFalse))
            .expect("expected a SelfFalse suppressor entry");
        assert_eq!(suppressor.tier_label, "quiet");
        let cleared = s
            .cleared
            .iter()
            .find(|d| matches!(d.reason, DroppedReason::SuppressedByFalse { .. }))
            .expect("expected a SuppressedByFalse cleared entry");
        assert_eq!(cleared.tier_label, "defaults");
    }

    #[test]
    fn trace_repeat_mode_emits_one_segment_per_occurrence() {
        let cfg = parse(
            r#"gcc {
                I "/a"
                I "/b"
            }"#,
        );
        let (resolved, trace) = resolve_with_trace(&cfg, "gcc", None, Path::new(".")).unwrap();
        assert_eq!(resolved.args.len(), 2);
        assert_eq!(trace.segments.len(), 2);
        for seg in &trace.segments {
            assert!(seg.mode_summary.as_deref().unwrap().contains("repeat"));
            assert!(matches!(seg.contributors[0].role, ContributorRole::Repeat));
        }
    }

    #[test]
    fn trace_marker_emission_records_marker_role() {
        let cfg = parse(
            r#"foo {
                +I "/extra"
            }"#,
        );
        let (resolved, trace) = resolve_with_trace(&cfg, "foo", None, Path::new(".")).unwrap();
        assert_eq!(resolved.args.len(), 1);
        let seg = &trace.segments[0];
        assert!(seg.mode_summary.as_deref().unwrap().contains("marker"));
        assert!(matches!(seg.contributors[0].role, ContributorRole::Marker));
    }

    #[test]
    fn trace_three_tier_records_chain_in_order() {
        let cfg = parse(
            r#"foo {
                grand {}
                parent extends="grand" {}
                child extends="parent" { x "from-child" }
            }"#,
        );
        let (_, trace) = resolve_with_trace(&cfg, "foo", Some("child"), Path::new(".")).unwrap();
        assert_eq!(trace.chain, vec!["grand", "parent", "child"]);
        assert_eq!(trace.selected_profile.as_deref(), Some("child"));
    }

    #[test]
    fn trace_env_winner_and_shadowed() {
        let cfg = parse(
            r#"foo {
                (env)A "default"
                p { (env)A "override" }
            }"#,
        );
        let (_, trace) = resolve_with_trace(&cfg, "foo", Some("p"), Path::new(".")).unwrap();
        assert_eq!(trace.env.len(), 1);
        let e = &trace.env[0];
        match &e.outcome {
            EnvOp::Set { name, .. } => assert_eq!(name, "A"),
            EnvOp::Unset { .. } => panic!("expected Set"),
        }
        assert_eq!(e.winner_tier_label, "p");
        assert_eq!(e.shadowed.len(), 1);
        assert_eq!(e.shadowed[0].tier_label, "defaults");
    }

    #[test]
    fn trace_cwd_resolved_carries_source_and_tier() {
        let cfg = parse(r#"foo cwd="/srv" {}"#);
        let (_, trace) = resolve_with_trace(&cfg, "foo", None, Path::new(".")).unwrap();
        match trace.cwd {
            CwdTrace::Resolved {
                source,
                tier,
                tier_label,
                inherited,
                ..
            } => {
                assert_eq!(source, "/srv");
                assert_eq!(tier, 0);
                assert_eq!(tier_label, "command");
                assert!(!inherited);
            }
            CwdTrace::Inherited => panic!("expected resolved cwd"),
        }
    }

    #[test]
    fn trace_three_tier_middle_loss_records_lost_middle() {
        // Three tiers all set the same flag. Single mode: defaults
        // wins position (earliest), child wins value (highest tier),
        // parent loses both — its candidate gets `LostMiddle` and
        // the segment's `dropped` lists it under MiddleTierLost.
        let cfg = parse(
            r#"foo {
                x "from-default"
                parent { x "from-parent" }
                child extends="parent" { x "from-child" }
            }"#,
        );
        let (resolved, trace) =
            resolve_with_trace(&cfg, "foo", Some("child"), Path::new(".")).unwrap();
        assert_eq!(resolved.args.len(), 1);
        assert_eq!(trace.segments.len(), 1);
        let seg = &trace.segments[0];
        // Two contributors (defaults position, child value); parent
        // is in `dropped` rather than `contributors`.
        assert_eq!(seg.contributors.len(), 2);
        assert_eq!(seg.dropped.len(), 1);
        let parent_drop = &seg.dropped[0];
        assert_eq!(parent_drop.tier_label, "parent");
        assert_eq!(parent_drop.rendered_value, "\"from-parent\"");
        assert!(matches!(parent_drop.reason, DroppedReason::MiddleTierLost));
    }

    #[test]
    fn trace_null_ghost_position_used_records_ghost_as_position_winner() {
        // `a #null` reserves position at idx 0 with no value. The
        // profile supplies the value at a later idx. Single mode
        // emits at idx 0 with the profile's value; the ghost is
        // recorded as the position contributor.
        let cfg = parse(
            r#"foo {
                a #null
                p { a "x" }
            }"#,
        );
        let (resolved, trace) = resolve_with_trace(&cfg, "foo", Some("p"), Path::new(".")).unwrap();
        assert_eq!(resolved.args.len(), 1);
        let seg = &trace.segments[0];
        // Two contributors: the ghost (PositionOnly, defaults tier)
        // and the profile entry (ValueOnly, p tier).
        assert_eq!(seg.contributors.len(), 2);
        let pos = seg
            .contributors
            .iter()
            .find(|c| matches!(c.role, ContributorRole::PositionOnly))
            .expect("expected a PositionOnly contributor for the ghost");
        assert_eq!(pos.tier_label, "defaults");
        let val = seg
            .contributors
            .iter()
            .find(|c| matches!(c.role, ContributorRole::ValueOnly))
            .expect("expected a ValueOnly contributor for the profile");
        assert_eq!(val.tier_label, "p");
    }
}
