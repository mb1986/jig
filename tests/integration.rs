//! End-to-end tests for the `jig` binary.
//!
//! Step 3 covers: missing config (exit 125), parse error (exit 125),
//! valid config silent success (exit 0), and `--list` output.

use std::fs;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

fn jig() -> Command {
    Command::cargo_bin("jig").expect("invariant: jig binary built by cargo test")
}

#[test]
fn missing_config_exits_125_with_helpful_diagnostic() {
    let dir = tempdir().unwrap();
    // Constrain the upward walk to the tempdir tree so the test
    // doesn't accidentally pick up a real jig.kdl in some ancestor
    // of the test runner's CWD.
    jig()
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .assert()
        .code(125)
        .stderr(predicate::str::contains("config file not found"))
        .stderr(predicate::str::contains("jig.kdl"))
        .stderr(predicate::str::contains(".jig.kdl"))
        .stderr(predicate::str::contains("from:"))
        .stderr(predicate::str::contains("up to:"));
}

#[test]
fn malformed_kdl_exits_125_with_parse_diagnostic() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo {\n").unwrap();
    jig()
        .current_dir(dir.path())
        .assert()
        .code(125)
        .stderr(predicate::str::contains("parse").or(predicate::str::contains("KDL")));
}

#[test]
fn flag_with_multiple_values_exits_125() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "foo {\n    host \"a\" \"b\"\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .assert()
        .code(125)
        .stderr(predicate::str::contains("multiple values"));
}

#[test]
fn property_on_node_exits_125() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo {\n    host port=8090\n}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .assert()
        .code(125)
        .stderr(predicate::str::contains("KDL properties"));
}

#[test]
fn dry_run_emits_resolved_line_on_stdout() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "llama-server \"serve\" {\n    host \"0.0.0.0\"\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("serve")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("llama-server --host 0.0.0.0"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn dot_jig_kdl_is_used_when_jig_kdl_absent() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join(".jig.kdl"), "foo\n").unwrap();
    // `--list` confirms the config loaded and validated cleanly.
    jig().current_dir(dir.path()).arg("--list").assert().code(0);
}

#[test]
fn jig_kdl_wins_over_dot_jig_kdl_when_both_exist() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo\n").unwrap();
    fs::write(dir.path().join(".jig.kdl"), "this is invalid kdl {").unwrap();
    // If `.jig.kdl` were preferred, we'd get a parse error and exit 125.
    jig().current_dir(dir.path()).arg("--list").assert().code(0);
}

#[test]
fn explicit_config_flag_skips_cwd_lookup() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join("custom.kdl");
    fs::write(&cfg, "foo\n").unwrap();
    // Run from a different directory that has no jig.kdl, pointing
    // explicitly at the custom file.
    let other = tempdir().unwrap();
    jig()
        .current_dir(other.path())
        .arg("--config")
        .arg(&cfg)
        .arg("--list")
        .assert()
        .code(0);
}

// --- §2.1 parent-directory traversal ---

#[test]
fn finds_config_in_parent_directory() {
    // jig.kdl lives at the tempdir root; jig runs from a subdirectory
    // and must walk upward to find it.
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "outer-cmd \"outer\" {\n}\n").unwrap();
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).unwrap();
    jig()
        .current_dir(&sub)
        .env("HOME", dir.path())
        .arg("--list")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("outer-cmd  (alias: outer)"));
}

#[test]
fn nearest_config_wins_when_ancestor_also_has_one() {
    // Both tempdir/jig.kdl and tempdir/sub/jig.kdl exist; running
    // from tempdir/sub picks the closer one.
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "outer-cmd \"outer\" {\n}\n").unwrap();
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join("jig.kdl"), "inner-cmd \"inner\" {\n}\n").unwrap();
    jig()
        .current_dir(&sub)
        .env("HOME", dir.path())
        .arg("--list")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("inner-cmd"))
        .stdout(predicate::str::contains("outer-cmd").not());
}

#[test]
fn dot_jig_kdl_used_in_ancestor_directory() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join(".jig.kdl"), "foo\n").unwrap();
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).unwrap();
    jig()
        .current_dir(&sub)
        .env("HOME", dir.path())
        .arg("--list")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("foo"));
}

#[test]
fn visible_beats_hidden_in_same_ancestor() {
    // SPEC §2.1: within a single directory, jig.kdl wins over
    // .jig.kdl. That precedence must apply at every level of the
    // upward walk, not only the starting CWD.
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo\n").unwrap();
    fs::write(dir.path().join(".jig.kdl"), "this is invalid kdl {").unwrap();
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).unwrap();
    // If the hidden file were preferred, we'd get a parse error (exit 125).
    jig()
        .current_dir(&sub)
        .env("HOME", dir.path())
        .arg("--list")
        .assert()
        .code(0);
}

#[test]
fn respects_home_boundary() {
    // Layout:
    //   tempdir/jig.kdl         (one level *above* HOME)
    //   tempdir/home/           (HOME)
    //   tempdir/home/proj/sub/  (CWD)
    //
    // With HOME=tempdir/home, the walk must stop after checking
    // tempdir/home and never reach tempdir/jig.kdl.
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "outside-home \"out\" {\n}\n").unwrap();
    let home = dir.path().join("home");
    let cwd = home.join("proj").join("sub");
    fs::create_dir_all(&cwd).unwrap();

    jig()
        .current_dir(&cwd)
        .env("HOME", &home)
        .arg("--list")
        .assert()
        .code(125)
        .stderr(predicate::str::contains("config file not found"));

    // Moving the config into HOME makes it reachable again.
    fs::write(home.join("jig.kdl"), "inside-home \"in\" {\n}\n").unwrap();
    jig()
        .current_dir(&cwd)
        .env("HOME", &home)
        .arg("--list")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("inside-home"));
}

#[test]
fn list_prints_command_alias_and_profiles() {
    // §7.1 format: `<name>  (alias: <alias>)`, `defaults: ...` (with
    // §2.5 prefix synthesis), and a `profiles:` block with per-profile
    // `args:` sub-lines.
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "llama-server \"serve\" {\n    host \"0.0.0.0\"\n    qwen-coder {\n        m \"/p\"\n    }\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--list")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("llama-server  (alias: serve)"))
        .stdout(predicate::str::contains("defaults: --host 0.0.0.0"))
        .stdout(predicate::str::contains("profiles:"))
        .stdout(predicate::str::contains("    qwen-coder"))
        .stdout(predicate::str::contains("      args: -m /p"));
}

#[test]
fn list_requires_config_to_exist() {
    let dir = tempdir().unwrap();
    jig()
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .arg("--list")
        .assert()
        .code(125)
        .stderr(predicate::str::contains("config file not found"));
}

// --- §2.9 constraint enforcement (Step 4) ---

#[test]
fn duplicate_command_name_without_alias_exits_125() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo {}\nfoo {}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .assert()
        .code(125)
        .stderr(predicate::str::contains("defined more than once"));
}

#[test]
fn duplicate_command_name_with_distinct_aliases_lists_both() {
    // Two top-level entries that share the binary name but have
    // distinct aliases — both must be reachable via their aliases.
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "llama-server \"serve1\" {\n    a #true\n}\nllama-server \"serve2\" {\n    b #true\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--list")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("llama-server  (alias: serve1)"))
        .stdout(predicate::str::contains("llama-server  (alias: serve2)"));
}

#[test]
fn alias_lookup_of_duplicated_command_runs() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "llama-server \"serve1\" {\n    a #true\n}\nllama-server \"serve2\" {\n    b #true\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("serve2")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("llama-server -b"));
}

#[test]
fn bare_name_lookup_of_duplicated_command_exits_125() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "llama-server \"serve1\" {\n    a #true\n}\nllama-server \"serve2\" {\n    b #true\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("llama-server")
        .assert()
        .code(125)
        .stderr(predicate::str::contains("ambiguous"))
        .stderr(predicate::str::contains("serve1"))
        .stderr(predicate::str::contains("serve2"));
}

#[test]
fn duplicate_alias_exits_125() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "llama-server \"serve\" {}\ngemma-server \"serve\" {}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .assert()
        .code(125)
        .stderr(predicate::str::contains("alias \"serve\""));
}

#[test]
fn cross_command_alias_collision_exits_125() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "serve {}\nllama-server \"serve\" {}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .assert()
        .code(125)
        .stderr(predicate::str::contains("collides"));
}

#[test]
fn self_alias_is_accepted() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo \"foo\" {}\n").unwrap();
    jig().current_dir(dir.path()).arg("--list").assert().code(0);
}

#[test]
fn duplicate_profile_in_command_exits_125() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "foo {\n    fast {}\n    fast {}\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .assert()
        .code(125)
        .stderr(predicate::str::contains("profile \"fast\""));
}

#[test]
fn repeated_default_flags_emit_in_order() {
    // gcc-style: same key twice in defaults resolves to repeat mode.
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "gcc {\n    I \"/usr/include\"\n    I \"/opt/include\"\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("gcc")
        .assert()
        .code(0)
        .stdout("gcc -I /usr/include -I /opt/include\n");
}

#[test]
fn append_marker_adds_to_single_default() {
    // The `+` prefix lets a profile add an occurrence without
    // overriding the default.
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "gcc {\n    I \"/usr/include\"\n    proj-extras {\n        +I \"/proj/include\"\n    }\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("gcc")
        .arg("proj-extras")
        .assert()
        .code(0)
        .stdout("gcc -I /usr/include -I /proj/include\n");
}

#[test]
fn profile_false_clears_multi_default_list() {
    // Profile-side `#false` wipes every default occurrence of the
    // key, regardless of marker.
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "gcc {\n    I \"/a\"\n    I \"/b\"\n    bare {\n        I #false\n    }\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("gcc")
        .arg("bare")
        .assert()
        .code(0)
        .stdout("gcc\n");
}

#[test]
fn append_marker_alone_exits_125() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo {\n    \"+\" \"v\"\n}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("foo")
        .assert()
        .code(125)
        .stderr(predicate::str::contains("empty after the `+`"));
}

#[test]
fn leading_dash_command_name_exits_125() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "\"-bad\" {}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .assert()
        .code(125)
        .stderr(predicate::str::contains("must not start with `-`"));
}

// --- §3.1 / §3.4 / §7.2 dry-run + resolution (Step 5) ---

#[test]
fn no_command_prints_help_and_exits_125() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo {}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .assert()
        .code(125)
        .stderr(predicate::str::contains("Usage:"));
}

#[test]
fn unknown_command_exits_125_with_did_you_mean() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "llama-server \"serve\" {\n    host \"0.0.0.0\"\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("llama-servr")
        .assert()
        .code(125)
        .stderr(predicate::str::contains("unknown command"))
        .stderr(predicate::str::contains("did you mean"));
}

#[test]
fn unknown_profile_exits_125_with_available_list() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "foo {\n    fast {}\n    slow {}\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("foo")
        .arg("medium")
        .assert()
        .code(125)
        .stderr(predicate::str::contains("unknown profile"))
        .stderr(predicate::str::contains("fast"))
        .stderr(predicate::str::contains("slow"));
}

#[test]
fn dry_run_resolves_profile_inheritance_end_to_end() {
    // End-to-end pipe: parse → validate → resolve → format → print.
    // `qwen-coder-large` inherits from `qwen-coder` and overrides
    // only `-m`. Defaults pass through; `-ngl 999` is inherited.
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        r#"llama-server "serve" {
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
"#,
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("serve")
        .arg("qwen-coder-large")
        .assert()
        .code(0)
        .stdout(predicate::str::contains(
            "llama-server --host 0.0.0.0 --port 8090 -m /models/qwen-coder-large.gguf -ngl 999",
        ));
}

#[test]
fn unknown_extends_parent_exits_125_with_help() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        r#"foo {
    parent {}
    child extends="paren" {}
}
"#,
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("foo")
        .assert()
        .code(125)
        .stderr(predicate::str::contains(
            "extends unknown profile \"paren\"",
        ))
        .stderr(predicate::str::contains("did you mean \"parent\""));
}

#[test]
fn extends_cycle_exits_125_with_cycle_path() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        r#"foo {
    a extends="b" {}
    b extends="a" {}
}
"#,
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("foo")
        .assert()
        .code(125)
        .stderr(predicate::str::contains("profile inheritance cycle"))
        .stderr(predicate::str::contains("a → b → a").or(predicate::str::contains("b → a → b")));
}

#[test]
fn dry_run_null_default_filled_by_profile_emits_at_default_position() {
    // §2.4.3 placeholder pattern end-to-end: `a #null` reserves
    // idx 0; the profile fills in the value; the merged occurrence
    // emits at the default's slot before `-b 123`.
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "command {\n    a #null\n    b 123\n\n    profile {\n        a 000\n    }\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("command")
        .arg("profile")
        .assert()
        .code(0)
        .stdout("command -a 000 -b 123\n");
}

#[test]
fn dry_run_null_default_alone_omits_flag() {
    // §2.4.3: with no profile filling in the placeholder, the flag
    // does not emit. The placeholder is structural-only.
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "command {\n    a #null\n    b 123\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("command")
        .assert()
        .code(0)
        .stdout("command -b 123\n");
}

#[test]
fn null_with_append_marker_exits_125() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo {\n    +a #null\n}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("foo")
        .assert()
        .code(125)
        .stderr(predicate::str::contains(
            "`+` append marker is not allowed on a `#null`",
        ));
}

#[test]
fn dry_run_for_spec_5_1_llama_server() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        r#"llama-server "serve" {
    host "0.0.0.0"
    port 8090
    c 32768
    flash-attn #true

    qwen-coder {
        m "/models/qwen-coder.gguf"
        -ngl 999
        -ts "0.5,0.5"
    }
}
"#,
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("serve")
        .arg("qwen-coder")
        .assert()
        .code(0)
        // shlex conservatively single-quotes values containing comma, so
        // `0.5,0.5` renders as `'0.5,0.5'`. Per §7.2 ("may be emitted
        // unquoted for readability"), unquoted is permission, not
        // requirement; the line is still copy-paste-faithful.
        .stdout(predicate::str::contains(
            "llama-server --host 0.0.0.0 --port 8090 -c 32768 --flash-attn -m /models/qwen-coder.gguf -ngl 999 -ts '0.5,0.5'",
        ));
}

#[test]
fn passthrough_args_appear_at_end() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo {\n    host \"x\"\n}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("foo")
        .arg("--")
        .arg("--extra")
        .arg("y")
        .assert()
        .code(0)
        // §3.1: `--` in the profile slot is consumed as the
        // "no profile" marker; the remaining tokens are
        // appended at the end of the resolved command line.
        .stdout("foo --host x --extra y\n");
}

#[test]
fn double_dash_in_profile_slot_skips_profile_lookup() {
    // §3.1: a bare positional pass-through that happens not to be a
    // profile name would otherwise error as "unknown profile". The
    // no-profile marker (`--` in the profile slot) lets the user
    // skip the slot so the token reaches the child.
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "foo {\n    host \"x\"\n    fast {}\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("foo")
        .arg("--")
        .arg("not-a-profile")
        .assert()
        .code(0)
        .stdout("foo --host x not-a-profile\n");
}

#[test]
fn second_double_dash_after_no_profile_marker_is_preserved() {
    // §3.2: only the first `--` (in the profile slot) is consumed.
    // A subsequent `--` is in the pass-through region and survives.
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo {\n    host \"x\"\n}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("foo")
        .arg("--")
        .arg("--")
        .arg("bar")
        .assert()
        .code(0)
        .stdout("foo --host x -- bar\n");
}

#[test]
fn passthrough_hyphen_args_pass_through() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo {\n}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("foo")
        .arg("-x")
        .arg("--abc")
        .arg("-y")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("-x --abc -y"));
}

#[test]
fn dry_run_quotes_values_with_spaces() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "foo {\n    label \"hello world\"\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("foo")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("'hello world'"));
}

// --- §3.5 / §3.6 exec semantics (Step 6) ---

#[test]
fn exec_propagates_zero_exit() {
    let dir = tempdir().unwrap();
    // KDL v2 treats bare `true`/`false` as boolean keywords, so the
    // command name must be quoted.
    fs::write(dir.path().join("jig.kdl"), "\"true\" {\n}\n").unwrap();
    jig().current_dir(dir.path()).arg("true").assert().code(0);
}

#[test]
fn exec_propagates_non_zero_exit() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "\"false\" {\n}\n").unwrap();
    // /bin/false (or similar) exits 1; we propagate verbatim.
    jig().current_dir(dir.path()).arg("false").assert().code(1);
}

#[test]
fn exec_propagates_arbitrary_exit_code() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "sh {\n    -c \"exit 42\"\n}\n").unwrap();
    jig().current_dir(dir.path()).arg("sh").assert().code(42);
}

#[test]
fn exec_missing_binary_exits_127() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "no-such-binary-xyz123 {\n}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("no-such-binary-xyz123")
        .assert()
        .code(127)
        .stderr(predicate::str::contains("command not found"));
}

#[cfg(unix)]
#[test]
fn exec_non_executable_exits_126() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    let bin = dir.path().join("notexec");
    fs::write(&bin, "#!/bin/sh\necho hi\n").unwrap();
    // Create the file without executable bits.
    let mut perms = fs::metadata(&bin).unwrap().permissions();
    perms.set_mode(0o644);
    fs::set_permissions(&bin, perms).unwrap();

    fs::write(
        dir.path().join("jig.kdl"),
        format!("\"{}\" {{\n}}\n", bin.display()),
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg(bin.to_str().unwrap())
        .assert()
        .code(126)
        .stderr(predicate::str::contains("not executable"));
}

#[test]
fn dry_run_does_not_exec() {
    // With --dry-run, even a missing binary should NOT trigger a 127
    // exec error — we should print the resolved line and exit 0.
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "no-such-binary-xyz123 {\n}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("no-such-binary-xyz123")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("no-such-binary-xyz123"));
}

#[test]
fn exec_passes_through_passthrough_args() {
    let dir = tempdir().unwrap();
    // `printf '%s\n' a b c` exits 0 and prints each on its own line.
    fs::write(dir.path().join("jig.kdl"), "printf {\n    \"%s\\n\"\n}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("printf")
        // §3.1: `--` here is the no-profile marker (consumed),
        // so `a b c` reach the child as bare positionals.
        .arg("--")
        .arg("a")
        .arg("b")
        .arg("c")
        .assert()
        .code(0)
        .stdout("a\nb\nc\n");
}

// --- §3.4.1 pre-exec preview ---

#[test]
fn preview_line_appears_on_stderr_before_exec() {
    // `assert_cmd` captures stderr through a pipe, so `IsTerminal`
    // is false: the preview is emitted in plain text (no ANSI bold).
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "echo {\n    \"hello\"\n}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("echo")
        .assert()
        .code(0)
        .stdout("hello\n")
        .stderr("echo hello\n");
}

#[test]
fn preview_line_includes_passthrough_args() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "echo {\n    \"hello\"\n}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("echo")
        .arg("--")
        .arg("world")
        .assert()
        .code(0)
        .stdout("hello world\n")
        .stderr("echo hello world\n");
}

#[test]
fn quiet_long_flag_suppresses_preview() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "echo {\n    \"hello\"\n}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--quiet")
        .arg("echo")
        .assert()
        .code(0)
        .stdout("hello\n")
        .stderr(predicate::str::is_empty());
}

#[test]
fn quiet_short_flag_suppresses_preview() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "echo {\n    \"hello\"\n}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("-q")
        .arg("echo")
        .assert()
        .code(0)
        .stdout("hello\n")
        .stderr(predicate::str::is_empty());
}

#[test]
fn preview_does_not_double_print_under_dry_run() {
    // --dry-run already writes the resolved line to stdout; the
    // pre-exec preview must not also fire (it would duplicate the
    // output on stderr).
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "echo {\n    \"hello\"\n}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("echo")
        .assert()
        .code(0)
        .stdout("echo hello\n")
        .stderr(predicate::str::is_empty());
}

#[test]
fn preview_renders_env_prefix_when_env_vars_set() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "echo {\n    \"hi\"\n    (env)JIG_TEST_PREVIEW \"yes\"\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("echo")
        .assert()
        .code(0)
        .stdout("hi\n")
        .stderr("env JIG_TEST_PREVIEW=yes echo hi\n");
}

#[test]
fn preview_shell_quotes_values_with_spaces() {
    // The preview must reuse `--dry-run`'s shell-quoting so the
    // line stays copy-pasteable; an unquoted value with a space
    // would split into two argv tokens.
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "echo {\n    \"a b\"\n}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("echo")
        .assert()
        .code(0)
        .stdout("a b\n")
        .stderr("echo 'a b'\n");
}

#[test]
fn preview_suppressed_on_non_exec_paths() {
    // `--list` (and the other non-exec paths) never reach the
    // preview emitter; this test guards against future regressions
    // that might push the preview earlier in `run()`.
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "echo {\n    \"hi\"\n}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--list")
        .assert()
        .code(0)
        .stderr(predicate::str::is_empty());
}

// --- §7.1 list rendering, §3.4 --completions (Step 7) ---

#[test]
fn list_renders_spec_5_1_example_shape() {
    // The §7.1 example uses the §5.1 llama-server config. Snapshot
    // its rendered listing so future format changes are visible.
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        r#"llama-server "serve" {
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
"#,
    )
    .unwrap();
    let out = jig()
        .current_dir(dir.path())
        .arg("--list")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    insta::assert_snapshot!(stdout);
}

#[test]
fn list_shows_extends_on_inheriting_profile() {
    // §2.8.5: an inheriting profile renders as `<name> (extends
    // <parent>)` under the `profiles:` block. Plain (non-inheriting)
    // profiles render without the suffix.
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        r#"llama-server "serve" {
    host "0.0.0.0"

    qwen-coder {
        m "/models/qwen-coder.gguf"
        -ngl 999
    }

    qwen-coder-large extends="qwen-coder" {
        m "/models/qwen-coder-large.gguf"
    }
}
"#,
    )
    .unwrap();
    let out = jig()
        .current_dir(dir.path())
        .arg("--list")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("    qwen-coder\n"),
        "plain profile should render bare; stdout: {stdout:?}",
    );
    assert!(
        stdout.contains("    qwen-coder-large  (extends qwen-coder)\n"),
        "inheriting profile should show `(extends <parent>)`; stdout: {stdout:?}",
    );
    insta::assert_snapshot!(stdout);
}

#[test]
fn list_renders_full_profile_sub_block() {
    // §7.1: each profile gets its own optional `cwd:` / `env:` /
    // `args:` sub-lines. This exercises all three channels at the
    // profile level plus an inheriting profile, alongside the
    // corresponding command-level defaults.
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        r#"llama-server "serve" cwd="/srv" {
    host "0.0.0.0"
    (env)OLLAMA_HOST "0.0.0.0"
    (env)OLD #false

    qwen-coder {
        m "/models/qwen-coder.gguf"
        (env)CUDA_VISIBLE_DEVICES "0"
    }

    qwen-coder-large extends="qwen-coder" cwd="src" {
        m "/models/qwen-coder-large.gguf"
    }
}
"#,
    )
    .unwrap();
    let out = jig()
        .current_dir(dir.path())
        .arg("--list")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    insta::assert_snapshot!(stdout);
}

#[test]
fn list_renders_empty_profile_as_header_only() {
    // §7.1: "a profile with an empty body is shown as just its
    // header" — no `cwd:` / `env:` / `args:` sub-lines should appear
    // under it, and rendering must continue immediately with the next
    // sibling (or end the block) with no blank line in between.
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "foo {\n    bare {}\n    nonbare {\n        host \"x\"\n    }\n}\n",
    )
    .unwrap();
    let out = jig()
        .current_dir(dir.path())
        .arg("--list")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("    bare\n    nonbare\n"),
        "empty profile should render as just its header; stdout: {stdout:?}",
    );
    assert!(
        stdout.contains("      args: --host x"),
        "non-empty sibling profile should still get its args sub-line; \
         stdout: {stdout:?}",
    );
}

#[test]
fn list_separates_commands_with_blank_line() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "foo {\n    host \"x\"\n}\nbar {\n    port 9000\n}\n",
    )
    .unwrap();
    let out = jig()
        .current_dir(dir.path())
        .arg("--list")
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    // A blank line between command blocks: \n\n must occur.
    assert!(stdout.contains("\n\n"), "stdout was: {stdout:?}");
}

#[test]
fn list_omits_default_args_when_none() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo {\n    fast {}\n}\n").unwrap();
    let out = jig()
        .current_dir(dir.path())
        .arg("--list")
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(!stdout.contains("defaults:"));
    assert!(stdout.contains("profiles:"));
    assert!(stdout.contains("    fast"));
}

#[test]
fn completions_zsh_emits_non_empty_script() {
    // No config required (per Q1). Run from an empty directory.
    let dir = tempdir().unwrap();
    let out = jig()
        .current_dir(dir.path())
        .arg("--completions")
        .arg("zsh")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(!stdout.is_empty());
    // zsh completions start with `#compdef`.
    assert!(stdout.contains("#compdef"));
}

#[test]
fn completions_bash_emits_non_empty_script() {
    let dir = tempdir().unwrap();
    let out = jig()
        .current_dir(dir.path())
        .arg("--completions")
        .arg("bash")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(!stdout.is_empty());
    // bash completions reference `complete -F`.
    assert!(stdout.contains("complete"));
}

#[test]
fn completions_works_without_config_file() {
    // Confirms Q1: --completions does not load or require jig.kdl.
    let empty = tempdir().unwrap();
    jig()
        .current_dir(empty.path())
        .arg("--completions")
        .arg("fish")
        .assert()
        .code(0);
}

#[test]
fn completions_unknown_shell_exits_2() {
    // clap's value_enum rejects unknown shells; clap's parse error
    // exits with its own code (2), not jig's 125.
    let dir = tempdir().unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--completions")
        .arg("ksh")
        .assert()
        .failure();
}

#[test]
fn help_prints_and_exits_0() {
    jig()
        .arg("--help")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("Usage:"));
}

#[test]
fn version_flag_prints_version() {
    jig()
        .arg("--version")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("jig"));
}

// --- §3.4 dynamic completion (--list-commands / --list-profiles) ---

#[test]
fn list_commands_prints_unique_names_and_aliases() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "llama-server \"serve\" {}\nrsync \"sync\" {}\nfoo {}\n",
    )
    .unwrap();
    let out = jig()
        .current_dir(dir.path())
        .arg("--list-commands")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["llama-server", "serve", "rsync", "sync", "foo"]);
}

#[test]
fn list_commands_excludes_duplicated_bare_names() {
    // A bare command name that appears twice is not a valid lookup
    // key (typing it errors as ambiguous), so it must not be a
    // completion candidate. Aliases stay.
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "llama-server \"a\" {}\nllama-server \"b\" {}\nfoo {}\n",
    )
    .unwrap();
    let out = jig()
        .current_dir(dir.path())
        .arg("--list-commands")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["a", "b", "foo"]);
}

#[test]
fn list_commands_with_no_config_exits_0_silent() {
    // Completion must never break mid-tab: a missing config
    // produces empty stdout, empty stderr, exit 0.
    let dir = tempdir().unwrap();
    jig()
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .arg("--list-commands")
        .assert()
        .code(0)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::is_empty());
}

#[test]
fn list_commands_with_malformed_config_exits_0_silent() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "this is not valid kdl {").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--list-commands")
        .assert()
        .code(0)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::is_empty());
}

#[test]
fn list_profiles_via_alias_prints_profiles() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "llama-server \"serve\" {\n    qwen-coder {}\n    llama3 {}\n}\n",
    )
    .unwrap();
    let out = jig()
        .current_dir(dir.path())
        .arg("--list-profiles")
        .arg("serve")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["qwen-coder", "llama3"]);
}

#[test]
fn list_profiles_for_duplicated_bare_name_is_silent() {
    // A duplicated bare name has no unique profile set; the user
    // must invoke via an alias. Completion emits nothing.
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "llama-server \"a\" { p1 {} }\nllama-server \"b\" { p2 {} }\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--list-profiles")
        .arg("llama-server")
        .assert()
        .code(0)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::is_empty());
}

#[test]
fn list_profiles_for_unknown_command_is_silent() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo { fast {} }\n").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--list-profiles")
        .arg("bogus")
        .assert()
        .code(0)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::is_empty());
}

#[test]
fn list_profiles_with_no_config_exits_0_silent() {
    let dir = tempdir().unwrap();
    jig()
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .arg("--list-profiles")
        .arg("anything")
        .assert()
        .code(0)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::is_empty());
}

#[test]
fn list_profiles_honors_explicit_config() {
    // Completion scripts forward `--config <PATH>` so candidates
    // reflect the user-chosen file, not the cwd-discovered one.
    let cfg_dir = tempdir().unwrap();
    let cfg_path = cfg_dir.path().join("custom.kdl");
    fs::write(
        &cfg_path,
        "foo {\n    from-custom {}\n    also-from-custom {}\n}\n",
    )
    .unwrap();
    let other = tempdir().unwrap();
    let out = jig()
        .current_dir(other.path())
        .arg("--config")
        .arg(&cfg_path)
        .arg("--list-profiles")
        .arg("foo")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["from-custom", "also-from-custom"]);
}

#[test]
fn completions_fish_emits_non_empty_script() {
    let dir = tempdir().unwrap();
    let out = jig()
        .current_dir(dir.path())
        .arg("--completions")
        .arg("fish")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(!stdout.is_empty());
    assert!(stdout.contains("complete -c jig"));
}

#[test]
fn completions_each_shell_references_dynamic_flags() {
    // Snapshot-style sanity: each script must dispatch to the
    // candidate-emitting flags so completion is truly dynamic.
    for shell in ["zsh", "bash", "fish"] {
        let out = jig().arg("--completions").arg(shell).output().unwrap();
        assert_eq!(out.status.code(), Some(0), "shell {shell} failed");
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert!(
            stdout.contains("--list-commands"),
            "{shell} script missing --list-commands"
        );
        assert!(
            stdout.contains("--list-profiles"),
            "{shell} script missing --list-profiles"
        );
    }
}

// --- §3.4 completion-script behavior (regression tests) ---
//
// jig flags only apply before the first positional. After that,
// every token — including hyphen-prefixed ones — is command,
// profile, or pass-through context. The completion scripts must
// honor this split, otherwise tabbing past the command name
// re-offers jig's own flags as candidates.
//
// Shells are skipped if not on PATH. zsh ships with macOS;
// bash ships with both runners; ubuntu-latest also has zsh
// available.

fn shell_present(shell: &str) -> bool {
    std::process::Command::new(shell)
        .arg("-c")
        .arg(":")
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

const ZSH_DRIVER: &str = r#"
typeset -ga TRACE
_arguments() { TRACE+=("arguments") }
_describe()  { shift 2; for c in "$@"; do TRACE+=("describe:$c"); done }
_files()     { TRACE+=("files") }
source "$1"
shift
typeset -ga words=("$@")
typeset -gi CURRENT=${#words[@]}
TRACE=()
_jig 2>/dev/null
print -r -- "${(j:|:)TRACE}"
"#;

fn zsh_trace(words: &[&str]) -> String {
    let script = std::env::current_dir()
        .unwrap()
        .join("src/completions/jig.zsh");
    let mut cmd = std::process::Command::new("zsh");
    cmd.arg("-fc").arg(ZSH_DRIVER).arg("driver").arg(&script);
    for w in words {
        cmd.arg(w);
    }
    let out = cmd.output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn zsh_completion_pass_through_hyphen_after_positional_takes_files_branch() {
    if !shell_present("zsh") {
        eprintln!("skipping: zsh not on PATH");
        return;
    }
    // `jig serve -<TAB>` — `serve` is positional, `-` would be
    // pass-through. Must short-circuit to _files; must NOT call
    // _arguments (which would re-offer jig's own flags).
    let trace = zsh_trace(&["jig", "serve", "-"]);
    assert_eq!(trace, "files", "expected `files` only; got `{trace}`");
}

#[test]
fn zsh_completion_pass_through_hyphen_after_two_positionals_takes_files_branch() {
    if !shell_present("zsh") {
        eprintln!("skipping: zsh not on PATH");
        return;
    }
    let trace = zsh_trace(&["jig", "serve", "qwen-coder", "--"]);
    assert_eq!(trace, "files", "expected `files` only; got `{trace}`");
}

#[test]
fn zsh_completion_initial_hyphen_offers_jig_flags() {
    if !shell_present("zsh") {
        eprintln!("skipping: zsh not on PATH");
        return;
    }
    // `jig -<TAB>` — no positional yet, must call _arguments
    // (which has the jig flag specs).
    let trace = zsh_trace(&["jig", "-"]);
    assert!(
        trace.contains("arguments"),
        "expected `arguments`; got `{trace}`"
    );
    assert!(
        !trace.contains("files"),
        "must not short-circuit to _files at flag position; got `{trace}`"
    );
}

#[test]
fn zsh_completion_initial_empty_dispatches_to_arguments() {
    if !shell_present("zsh") {
        eprintln!("skipping: zsh not on PATH");
        return;
    }
    let trace = zsh_trace(&["jig", ""]);
    assert!(
        trace.contains("arguments"),
        "expected `arguments`; got `{trace}`"
    );
}

const BASH_DRIVER: &str = r#"
source "$1"
shift
COMP_WORDS=("$@")
COMP_CWORD=$(( ${#COMP_WORDS[@]} - 1 ))
COMPREPLY=()
_jig 2>/dev/null
printf "%s\n" "${COMPREPLY[@]}"
"#;

fn bash_reply(words: &[&str]) -> Vec<String> {
    let script = std::env::current_dir()
        .unwrap()
        .join("src/completions/jig.bash");
    let mut cmd = std::process::Command::new("bash");
    cmd.arg("-c").arg(BASH_DRIVER).arg("driver").arg(&script);
    for w in words {
        cmd.arg(w);
    }
    let out = cmd.output().unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

const JIG_FLAGS: &[&str] = &[
    "-h",
    "--help",
    "-V",
    "--version",
    "-l",
    "--list",
    "-n",
    "--dry-run",
    "--config",
];

#[test]
fn bash_completion_does_not_offer_jig_flags_after_positional() {
    if !shell_present("bash") {
        eprintln!("skipping: bash not on PATH");
        return;
    }
    let reply = bash_reply(&["jig", "serve", "-"]);
    for flag in JIG_FLAGS {
        assert!(
            !reply.iter().any(|x| x == flag),
            "must not offer jig flag `{flag}` after positional; got {reply:?}"
        );
    }
}

#[test]
fn bash_completion_does_not_offer_jig_flags_after_two_positionals() {
    if !shell_present("bash") {
        eprintln!("skipping: bash not on PATH");
        return;
    }
    let reply = bash_reply(&["jig", "serve", "qwen-coder", "--"]);
    for flag in JIG_FLAGS {
        assert!(
            !reply.iter().any(|x| x == flag),
            "must not offer jig flag `{flag}` after positional; got {reply:?}"
        );
    }
}

#[test]
fn bash_completion_initial_hyphen_offers_jig_flags() {
    if !shell_present("bash") {
        eprintln!("skipping: bash not on PATH");
        return;
    }
    let reply = bash_reply(&["jig", "-"]);
    for flag in ["--help", "--config", "--list", "--dry-run", "--version"] {
        assert!(
            reply.iter().any(|x| x == flag),
            "expected jig flag `{flag}`; got {reply:?}"
        );
    }
}

// --- tilde expansion in --config value (regression) ---
//
// Shells only expand `~` at parse time on unquoted words. The value
// captured from the command line (zsh `$words`, bash `$COMP_WORDS`,
// fish `commandline -opc`) keeps `~` literal, so the completion
// scripts must re-expand before forwarding to jig. Without that,
// `jig --config ~/cfg.kdl <TAB>` silently returns no candidates.

// Mocks the state-machine far enough to enter the `command` branch,
// then captures `config_args` from _jig's scope into TRACE via a
// `_describe` stand-in (dynamic scoping makes the local visible).
const ZSH_CAPTURE_DRIVER: &str = r#"
typeset -ga TRACE
_arguments() { state="command" }
_describe()  { TRACE+=("${config_args[*]}") }
_files()     { :; }
source "$1"
shift
typeset -ga words=("$@")
typeset -gi CURRENT=${#words[@]}
TRACE=()
_jig 2>/dev/null
print -r -- "${TRACE[1]}"
"#;

fn zsh_captured_config_args(home: &str, words: &[&str]) -> String {
    let script = std::env::current_dir()
        .unwrap()
        .join("src/completions/jig.zsh");
    let mut cmd = std::process::Command::new("zsh");
    cmd.env("HOME", home);
    cmd.arg("-fc")
        .arg(ZSH_CAPTURE_DRIVER)
        .arg("driver")
        .arg(&script);
    for w in words {
        cmd.arg(w);
    }
    let out = cmd.output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn zsh_completion_expands_tilde_slash_in_config_value() {
    if !shell_present("zsh") {
        eprintln!("skipping: zsh not on PATH");
        return;
    }
    let captured = zsh_captured_config_args("/fake/home", &["jig", "--config", "~/cfg.kdl", ""]);
    assert_eq!(captured, "--config /fake/home/cfg.kdl");
}

#[test]
fn zsh_completion_expands_tilde_slash_in_config_equals_value() {
    if !shell_present("zsh") {
        eprintln!("skipping: zsh not on PATH");
        return;
    }
    let captured = zsh_captured_config_args("/fake/home", &["jig", "--config=~/cfg.kdl", ""]);
    assert_eq!(captured, "--config=/fake/home/cfg.kdl");
}

#[test]
fn zsh_completion_leaves_globs_in_config_value_literal() {
    // Tilde expansion must not drag in glob expansion. A literal `*`
    // in the path should reach jig unchanged (jig will then fail to
    // open the file and the completion comes back empty, which is
    // the same as today — but the path is forwarded verbatim).
    if !shell_present("zsh") {
        eprintln!("skipping: zsh not on PATH");
        return;
    }
    let captured = zsh_captured_config_args("/fake/home", &["jig", "--config", "~/cfg/*.kdl", ""]);
    assert_eq!(captured, "--config /fake/home/cfg/*.kdl");
}

// As `zsh_captured_config_args` but also returns whatever the driver
// wrote to stderr — used to assert completion stays silent.
fn zsh_captured_config_args_with_stderr(home: &str, words: &[&str]) -> (String, String) {
    let script = std::env::current_dir()
        .unwrap()
        .join("src/completions/jig.zsh");
    let mut cmd = std::process::Command::new("zsh");
    cmd.env("HOME", home);
    cmd.arg("-fc")
        .arg(ZSH_CAPTURE_DRIVER)
        .arg("driver")
        .arg(&script);
    for w in words {
        cmd.arg(w);
    }
    let out = cmd.output().unwrap();
    (
        String::from_utf8(out.stdout).unwrap().trim().to_string(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

#[test]
fn zsh_completion_unknown_tilde_user_stays_literal_and_silent() {
    // `${~var}` would expand `~typo_xyz/...` AND write "no such user"
    // to stderr on unknown accounts — noisy during tab-completion.
    // The script intentionally handles only `~/` and bare `~` to
    // avoid that. Confirm the path passes through unchanged and
    // nothing leaks to stderr.
    if !shell_present("zsh") {
        eprintln!("skipping: zsh not on PATH");
        return;
    }
    let (captured, stderr) = zsh_captured_config_args_with_stderr(
        "/fake/home",
        &["jig", "--config", "~typo_xyz/x.kdl", ""],
    );
    assert_eq!(captured, "--config ~typo_xyz/x.kdl");
    assert!(stderr.is_empty(), "expected silent stderr; got `{stderr}`");
}

// Drives _jig with a shell-function `jig` stand-in. The stand-in
// encodes every arg it received (config_args followed by
// --list-commands) into a single candidate token written to stdout,
// which is what we then inspect via COMPREPLY.
const BASH_CAPTURE_DRIVER: &str = r#"
source "$1"
shift
jig() {
    local out="" a
    for a in "$@"; do
        out+="${a//[[:space:]]/_}@"
    done
    echo "captured:${out}"
}
COMP_WORDS=("$@")
COMP_CWORD=$(( ${#COMP_WORDS[@]} - 1 ))
COMPREPLY=()
_jig 2>/dev/null
printf "%s\n" "${COMPREPLY[@]}"
"#;

fn bash_captured_config_args(home: &str, words: &[&str]) -> String {
    let script = std::env::current_dir()
        .unwrap()
        .join("src/completions/jig.bash");
    let mut cmd = std::process::Command::new("bash");
    cmd.env("HOME", home);
    cmd.arg("-c")
        .arg(BASH_CAPTURE_DRIVER)
        .arg("driver")
        .arg(&script);
    for w in words {
        cmd.arg(w);
    }
    let out = cmd.output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn bash_completion_expands_tilde_slash_in_config_value() {
    if !shell_present("bash") {
        eprintln!("skipping: bash not on PATH");
        return;
    }
    let captured = bash_captured_config_args("/fake/home", &["jig", "--config", "~/cfg.kdl", ""]);
    assert_eq!(
        captured,
        "captured:--config@/fake/home/cfg.kdl@--list-commands@"
    );
}

#[test]
fn bash_completion_expands_tilde_slash_in_config_equals_value() {
    if !shell_present("bash") {
        eprintln!("skipping: bash not on PATH");
        return;
    }
    let captured = bash_captured_config_args("/fake/home", &["jig", "--config=~/cfg.kdl", ""]);
    assert_eq!(
        captured,
        "captured:--config=/fake/home/cfg.kdl@--list-commands@"
    );
}

#[test]
fn bash_completion_leaves_globs_in_config_value_literal() {
    // Mirror of the zsh glob test: a literal `*` in the path must
    // reach jig unchanged, with no IFS word-splitting or globbing
    // from the `compgen -W "$out"` round-trip.
    if !shell_present("bash") {
        eprintln!("skipping: bash not on PATH");
        return;
    }
    let captured = bash_captured_config_args("/fake/home", &["jig", "--config", "~/cfg/*.kdl", ""]);
    assert_eq!(
        captured,
        "captured:--config@/fake/home/cfg/*.kdl@--list-commands@"
    );
}

#[test]
fn bash_completion_passes_absolute_config_path_through_unchanged() {
    // Exercise the `*) printf '%s' "$1"` fallback in _jig_expand_tilde:
    // a path without a leading `~` must not be rewritten.
    if !shell_present("bash") {
        eprintln!("skipping: bash not on PATH");
        return;
    }
    let captured =
        bash_captured_config_args("/fake/home", &["jig", "--config", "/etc/jig.kdl", ""]);
    assert_eq!(captured, "captured:--config@/etc/jig.kdl@--list-commands@");
}

#[test]
fn dry_run_value_with_embedded_nul_byte_exits_125() {
    // A NUL byte cannot be shell-quoted, so --dry-run should refuse
    // rather than emit something that can't be copy-pasted. Since
    // a NUL also can't survive argv into a child process, this
    // failure mode is checked here at the dry-run layer.
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "foo {\n    label \"hello\\u{0}world\"\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("foo")
        .assert()
        .code(125)
        .stderr(predicate::str::contains("NUL byte"));
}

// --- §2.10 / §2.11 / §3.6 environment variables ---

#[test]
fn dry_run_renders_env_prefix_when_present() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        r#"llama-server "serve" {
    host "0.0.0.0"
    (env)OLLAMA_HOST "1.2.3.4"
    (env)CUDA_VISIBLE_DEVICES "0,1"
}
"#,
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("serve")
        .assert()
        .code(0)
        // shlex single-quotes the `0,1` value because the comma is
        // a shell metacharacter; only the value is wrapped, leaving
        // the K= prefix bare so the assignment shape is preserved.
        .stdout(predicate::str::starts_with(
            "env OLLAMA_HOST=1.2.3.4 CUDA_VISIBLE_DEVICES='0,1' llama-server",
        ));
}

#[test]
fn dry_run_emits_unsets_first() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        r#"foo {
    (env)A "1"
    sandbox {
        (env)PATH #false
        (env)B "2"
    }
}
"#,
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("foo")
        .arg("sandbox")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("env -u PATH A=1 B=2 foo"));
}

#[test]
fn dry_run_no_env_unchanged() {
    // Without env declarations, the dry-run line is byte-identical
    // to a config without env support — no `env` prefix.
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo {\n    host \"x\"\n}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("foo")
        .assert()
        .code(0)
        .stdout("foo --host x\n");
}

#[test]
fn list_includes_env_line_for_command_with_env_defaults() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        r#"foo {
    (env)OLLAMA_HOST "1.2.3.4"
    (env)OLD #false
    host "0.0.0.0"
}
"#,
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--list")
        .assert()
        .code(0)
        .stdout(predicate::str::contains(
            "env:      -u OLD OLLAMA_HOST=1.2.3.4",
        ))
        .stdout(predicate::str::contains("defaults: --host 0.0.0.0"));
}

#[test]
fn list_omits_env_line_when_no_env_defaults() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo {\n    host \"x\"\n}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--list")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("env:").not());
}

#[test]
fn env_set_reaches_child_default_and_profile_paths() {
    // Use `printenv` to confirm the resolved env reached the child.
    // The variable name is a positional default on the jig command,
    // so the resolved argv is `printenv JIG_TEST_FOO`. We then
    // assert on the printed value.
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        r#"printenv "show-foo" {
    "JIG_TEST_FOO"
    (env)JIG_TEST_FOO "default-value"
    overridden {
        (env)JIG_TEST_FOO "profile-value"
    }
}
"#,
    )
    .unwrap();

    // Defaults: FOO is set by jig.
    jig()
        .current_dir(dir.path())
        .arg("show-foo")
        .assert()
        .code(0)
        .stdout("default-value\n");

    // Profile overrides the default.
    jig()
        .current_dir(dir.path())
        .arg("show-foo")
        .arg("overridden")
        .assert()
        .code(0)
        .stdout("profile-value\n");
}

#[test]
fn env_unset_removes_inherited_var_from_child() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        r#"printenv "show-bar" {
    "JIG_TEST_BAR"
    sandbox {
        (env)JIG_TEST_BAR #false
    }
}
"#,
    )
    .unwrap();

    // Without the profile: BAR is whatever the parent set.
    jig()
        .current_dir(dir.path())
        .env("JIG_TEST_BAR", "from-parent")
        .arg("show-bar")
        .assert()
        .code(0)
        .stdout("from-parent\n");

    // With the unset profile: printenv exits non-zero because the
    // requested variable does not exist in the child's environment.
    jig()
        .current_dir(dir.path())
        .env("JIG_TEST_BAR", "from-parent")
        .arg("show-bar")
        .arg("sandbox")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty());
}

#[test]
fn dry_run_preserves_numeric_env_value_source_repr() {
    // Integers/floats round-trip as their KDL source text and need
    // no shell quoting. Pin the unquoted form so a future format
    // change doesn't silently re-quote them.
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        r#"foo {
    (env)PORT 8090
    (env)RATIO 0.5
}
"#,
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("foo")
        .assert()
        .code(0)
        .stdout("env PORT=8090 RATIO=0.5 foo\n");
}

#[test]
fn env_unknown_annotation_exits_125() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo {\n  (cwd)dir \"/x\"\n}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--list")
        .assert()
        .code(125)
        .stderr(predicate::str::contains("unknown type annotation"));
}

#[test]
fn env_invalid_name_exits_125() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "foo {\n  (env)\"FOO-BAR\" \"x\"\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--list")
        .assert()
        .code(125)
        .stderr(predicate::str::contains("env-var name"));
}

#[test]
fn env_duplicate_in_defaults_exits_125() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "foo {\n  (env)X \"1\"\n  (env)X \"2\"\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--list")
        .assert()
        .code(125)
        .stderr(predicate::str::contains("declared more than once"));
}

// --- §2.12 working directory ---

#[test]
fn cwd_dry_run_wraps_command_in_subshell() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "foo cwd=\"/opt/work\" {\n    host \"0.0.0.0\"\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("foo")
        .assert()
        .code(0)
        .stdout("(cd /opt/work && foo --host 0.0.0.0)\n");
}

#[test]
fn cwd_relative_resolves_to_config_dir() {
    let dir = tempdir().unwrap();
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo cwd=\"sub\" {}\n").unwrap();
    let expected = format!("(cd {}/sub && foo)\n", dir.path().display());
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("foo")
        .assert()
        .code(0)
        .stdout(expected);
}

#[test]
fn cwd_profile_overrides_command_in_dry_run() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "foo cwd=\"/cmd\" {\n    fast cwd=\"/profile\" {\n        -ngl 99\n    }\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("foo")
        .arg("fast")
        .assert()
        .code(0)
        .stdout("(cd /profile && foo -ngl 99)\n");
}

#[test]
fn cwd_with_env_compose_in_dry_run() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "foo cwd=\"/work\" {\n    (env)X \"1\"\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("foo")
        .assert()
        .code(0)
        .stdout("(cd /work && env X=1 foo)\n");
}

#[test]
fn cwd_list_shows_command_cwd_line() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "foo cwd=\"/work\" {\n    host \"0.0.0.0\"\n}\n",
    )
    .unwrap();
    let out = jig()
        .current_dir(dir.path())
        .arg("--list")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("  cwd:      /work\n"),
        "expected cwd line; stdout: {stdout:?}"
    );
}

#[test]
fn cwd_list_omits_cwd_line_when_unset() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo {\n    host \"x\"\n}\n").unwrap();
    let out = jig()
        .current_dir(dir.path())
        .arg("--list")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        !stdout.contains("cwd:"),
        "should not show cwd line; stdout: {stdout:?}"
    );
}

#[test]
fn cwd_list_shows_relative_path_verbatim() {
    // §7.1: --list shows the path as written in the config.
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo cwd=\"sub\" {}\n").unwrap();
    let out = jig()
        .current_dir(dir.path())
        .arg("--list")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("  cwd:      sub\n"),
        "expected relative path verbatim; stdout: {stdout:?}"
    );
}

#[test]
fn cwd_applied_to_spawn_with_pwd() {
    // End-to-end: invoke `pwd` with cwd set, and assert its stdout
    // is the resolved directory. This proves Command::current_dir
    // is wired in correctly. We use the bare name (resolved through
    // $PATH) rather than /bin/pwd because NixOS / Alpine place pwd
    // elsewhere, but every Unix ships some `pwd` on $PATH.
    let dir = tempdir().unwrap();
    let target = dir.path().join("work");
    fs::create_dir(&target).unwrap();
    fs::write(dir.path().join("jig.kdl"), "pwd cwd=\"work\" {}\n").unwrap();
    let out = jig()
        .current_dir(dir.path())
        .arg("--quiet")
        .arg("pwd")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    // macOS sometimes prefixes a tempdir with /private (a symlink).
    // pwd resolves it; we just want the trailing /work to match.
    assert!(
        stdout.trim().ends_with("/work"),
        "pwd output {stdout:?} should end in /work"
    );
}

#[test]
fn cwd_missing_directory_exits_125() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "\"/bin/true\" cwd=\"/this/does/not/exist\" {}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--quiet")
        .arg("/bin/true")
        .assert()
        .code(125)
        .stderr(predicate::str::contains("could not enter cwd"))
        .stderr(predicate::str::contains("/this/does/not/exist"));
}

#[test]
fn cwd_bad_value_exits_125_with_parse_diagnostic() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo cwd=42 {}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--list")
        .assert()
        .code(125)
        .stderr(predicate::str::contains("`cwd` value must be"));
}

#[test]
fn cwd_duplicate_exits_125() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo cwd=\"/a\" cwd=\"/b\" {}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--list")
        .assert()
        .code(125)
        .stderr(predicate::str::contains(
            "`cwd` is specified more than once",
        ));
}

#[test]
fn cwd_bare_config_filename_resolves_to_absolute_path() {
    // §7.2: the `(cd <dir> && ...)` wrapper must use the resolved
    // *absolute* path. When `--config jig.kdl` is passed with no
    // directory component, the implementation must prepend the
    // process CWD before resolving the relative `cwd=` value.
    let dir = tempdir().unwrap();
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo cwd=\"sub\" {}\n").unwrap();
    let out = jig()
        .current_dir(dir.path())
        .arg("--config")
        .arg("jig.kdl") // bare name, no directory component
        .arg("--dry-run")
        .arg("foo")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let trimmed = stdout.trim();
    let inside = trimmed
        .strip_prefix("(cd ")
        .and_then(|s| s.split(" && ").next())
        .expect("dry-run output should start with (cd <path> && ");
    assert!(
        inside.starts_with('/'),
        "cwd path inside (cd …) should be absolute; got {inside:?} (full output: {trimmed:?})"
    );
    assert!(
        inside.ends_with("/sub"),
        "cwd path should end in /sub; got {inside:?}"
    );
}

#[test]
fn cwd_dry_run_quotes_shell_metacharacters() {
    // §7.2: paths with `$`, `;`, `(`, `)`, etc. must be shell-quoted
    // so the (cd …) wrapper round-trips through a POSIX shell.
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "foo cwd=\"/work$with;meta(chars)\" {}\n",
    )
    .unwrap();
    let out = jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("foo")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(stdout, "(cd '/work$with;meta(chars)' && foo)\n");
}

#[test]
fn cwd_pre_exec_preview_shows_subshell_wrapper() {
    // §3.4.1: the pre-exec preview line (written to stderr before
    // spawning) uses the same shell-quoted form as `--dry-run`, so
    // a resolved cwd should appear there too. We use `pwd` so the
    // command actually succeeds; stderr should carry the preview.
    let dir = tempdir().unwrap();
    let work = dir.path().join("work");
    fs::create_dir(&work).unwrap();
    fs::write(dir.path().join("jig.kdl"), "pwd cwd=\"work\" {}\n").unwrap();
    let out = jig().current_dir(dir.path()).arg("pwd").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("(cd "),
        "preview should include `(cd …)`; stderr: {stderr:?}",
    );
    assert!(
        stderr.contains("/work"),
        "preview should mention /work; stderr: {stderr:?}",
    );
}

#[test]
fn cwd_pre_exec_preview_suppressed_under_quiet() {
    // -q suppresses the preview; even with a cwd set, stderr stays
    // empty (apart from anything the child writes — `pwd` writes
    // only to stdout).
    let dir = tempdir().unwrap();
    let work = dir.path().join("work");
    fs::create_dir(&work).unwrap();
    fs::write(dir.path().join("jig.kdl"), "pwd cwd=\"work\" {}\n").unwrap();
    let out = jig()
        .current_dir(dir.path())
        .arg("--quiet")
        .arg("pwd")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        !stderr.contains("(cd "),
        "preview should be suppressed under -q; stderr: {stderr:?}",
    );
}

#[test]
fn cwd_with_embedded_nul_exits_125_not_panics() {
    // KDL `\u{0}` escapes can put a NUL byte into the `cwd=` string.
    // Previously this hit an `expect()` in the dry-run rendering and
    // panicked the binary; it must now surface as a typed error and
    // exit 125 (a jig-configuration failure), never as a panic.
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo cwd=\"a\\u{0}b\" {}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--dry-run")
        .arg("foo")
        .assert()
        .code(125)
        .stderr(predicate::str::contains("NUL byte"))
        .stderr(predicate::str::contains("panicked").not());
}

#[test]
fn cwd_with_embedded_nul_live_exec_exits_125_not_panics() {
    // Parallel to the --dry-run check above, but exercises the
    // exec.rs path: a NUL in the cwd should be flagged as
    // CwdContainsNul before the spawn, with no panic.
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "pwd cwd=\"a\\u{0}b\" {}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--quiet")
        .arg("pwd")
        .assert()
        .code(125)
        .stderr(predicate::str::contains("NUL byte"))
        .stderr(predicate::str::contains("panicked").not());
}

#[test]
fn cwd_list_shows_profile_level_cwd() {
    // §7.1: profile-level `cwd=` appears as a `cwd:` sub-line under
    // the profile, mirroring the command-level rendering. Path is
    // shown as written in the config.
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "foo {\n    fast cwd=\"/profile-only\" {}\n}\n",
    )
    .unwrap();
    let out = jig()
        .current_dir(dir.path())
        .arg("--list")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("      cwd:  /profile-only\n"),
        "profile-level cwd should appear under profile; stdout: {stdout:?}",
    );
    // No command-level cwd was set, so no command-level cwd line.
    assert!(
        !stdout.contains("  cwd:      "),
        "no command-level cwd, no command-level cwd: line; stdout: {stdout:?}",
    );
}
