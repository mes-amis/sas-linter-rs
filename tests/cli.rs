//! End-to-end CLI invocation tests.
//!
//! Spawn the built `sas-lint` binary against fixtures and assert on the
//! observable contract: stdout, stderr, exit code. Exit-code semantics
//! mirror the Ruby gem:
//!     0 — no findings
//!     1 — findings reported
//!     2 — misuse / file-not-found / unknown rule

use std::path::PathBuf;
use std::process::Command;

fn bin() -> PathBuf {
    // Cargo sets CARGO_BIN_EXE_<name> for binaries declared in [[bin]].
    PathBuf::from(env!("CARGO_BIN_EXE_sas-lint"))
}

fn fixture(rule: &str, leaf: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/lints")
        .join(rule)
        .join(leaf)
}

#[test]
fn list_rules_exits_zero_and_prints_each_rule() {
    let out = Command::new(bin()).arg("--list-rules").output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("trailing_whitespace"));
    assert!(stdout.contains("malformed_if_condition"));
    assert!(stdout.contains("[autofix]"));
}

#[test]
fn lint_with_findings_exits_1() {
    let path = fixture("malformed_if_condition", "lint.sas");
    let out = Command::new(bin())
        .args(["--rules", "malformed_if_condition"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("malformed_if_condition"));
}

#[test]
fn clean_input_exits_0() {
    let path = fixture("malformed_if_condition", "clean.sas");
    let out = Command::new(bin())
        .args(["--rules", "malformed_if_condition"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn unknown_rule_exits_nonzero() {
    let out = Command::new(bin())
        .args(["--rules", "does_not_exist"])
        .arg(fixture("trailing_whitespace", "clean.sas"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn missing_file_exits_2() {
    let out = Command::new(bin())
        .args(["--rules", "trailing_whitespace"])
        .arg("/tmp/nope-does-not-exist-12345.sas")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}
