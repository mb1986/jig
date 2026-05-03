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

// --- §2.9 constraint enforcement (Step 4) ---

#[test]
fn duplicate_command_name_exits_125() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("jig.kdl"), "foo {}\nfoo {}\n").unwrap();
    jig()
        .current_dir(dir.path())
        .assert()
        .code(125)
        .stderr(predicate::str::contains("defined more than once"));
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
fn duplicate_flag_key_exits_125() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "foo {\n    host \"a\"\n    host \"b\"\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .assert()
        .code(125)
        .stderr(predicate::str::contains("--host"));
}

#[test]
fn resolved_form_flag_collision_exits_125() {
    // `host "a"` and `--host "b"` both resolve to --host.
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("jig.kdl"),
        "foo {\n    host \"a\"\n    --host \"b\"\n}\n",
    )
    .unwrap();
    jig()
        .current_dir(dir.path())
        .assert()
        .code(125)
        .stderr(predicate::str::contains("--host"));
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
        // §3.2: `--` is preserved verbatim in pass-through.
        .stdout(predicate::str::contains("foo --host x"))
        .stdout(predicate::str::contains("--extra y"));
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
        // `--` so `a` is treated as pass-through rather than profile.
        .arg("--")
        .arg("a")
        .arg("b")
        .arg("c")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("a\nb\nc\n"));
}
