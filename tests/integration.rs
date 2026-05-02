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
    jig()
        .current_dir(dir.path())
        .assert()
        .code(125)
        .stderr(predicate::str::contains("config file not found"))
        .stderr(predicate::str::contains("./jig.kdl"))
        .stderr(predicate::str::contains("./.jig.kdl"));
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
fn valid_config_exits_0_silently() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "llama-server \"serve\" {\n    host \"0.0.0.0\"\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .assert()
        .code(0)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::is_empty());
}

#[test]
fn dot_jig_kdl_is_used_when_jig_kdl_absent() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join(".jig.kdl"), "foo\n").unwrap();
    jig().current_dir(dir.path()).assert().code(0);
}

#[test]
fn jig_kdl_wins_over_dot_jig_kdl_when_both_exist() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo\n").unwrap();
    fs::write(dir.path().join(".jig.kdl"), "this is invalid kdl {").unwrap();
    // If `.jig.kdl` were preferred, we'd get a parse error and exit 125.
    jig().current_dir(dir.path()).assert().code(0);
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
        .assert()
        .code(0);
}

#[test]
fn list_prints_command_alias_and_profiles() {
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
        .stdout(predicate::str::contains("llama-server"))
        .stdout(predicate::str::contains("alias: serve"))
        .stdout(predicate::str::contains("profile qwen-coder"))
        .stdout(predicate::str::contains("inferred:host"));
}

#[test]
fn list_requires_config_to_exist() {
    let dir = tempdir().unwrap();
    jig()
        .current_dir(dir.path())
        .arg("--list")
        .assert()
        .code(125)
        .stderr(predicate::str::contains("config file not found"));
}
