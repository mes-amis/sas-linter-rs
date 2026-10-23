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

fn synth_tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sas-lint-synth-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn synthesize_writes_one_csv_per_source_into_output_dir() {
    let tmp = synth_tmp("dir");
    let a = tmp.join("ALGO_A_V1.0.sas");
    let b = tmp.join("ALGO_B.txt");
    std::fs::write(&a, "x = x;\nif y in (1, 2) and z gt 3 then r = 1;\n").unwrap();
    std::fs::write(&b, "if q = 'A' then s = 1;\n").unwrap();
    let out_dir = tmp.join("out");

    let out = Command::new(bin())
        .args(["synthesize", "--size", "25", "-o"])
        .arg(&out_dir)
        .arg(&a)
        .arg(&b)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let csv_a = std::fs::read_to_string(out_dir.join("ALGO_A_V1.0.csv")).unwrap();
    let mut lines = csv_a.lines();
    assert_eq!(lines.next(), Some("x,y,z"));
    assert_eq!(lines.count(), 25);
    let csv_b = std::fs::read_to_string(out_dir.join("ALGO_B.csv")).unwrap();
    assert!(csv_b.starts_with("q\n"));
    assert!(String::from_utf8_lossy(&out.stdout).contains("Wrote 25 row(s) over 3 input(s)"));
}

#[test]
fn synthesize_is_deterministic_and_accepts_a_csv_file_target() {
    let tmp = synth_tmp("file");
    let src = tmp.join("algo.sas");
    std::fs::write(&src, "if sum(a1, a2) ge 2 or b = 4 then c = 1;\n").unwrap();
    let run = |name: &str| {
        let target = tmp.join(name);
        let out = Command::new(bin())
            .args(["synthesize", "--seed", "9", "-o"])
            .arg(&target)
            .arg(&src)
            .output()
            .unwrap();
        assert!(out.status.success());
        std::fs::read_to_string(target).unwrap()
    };
    assert_eq!(run("one.csv"), run("two.csv"));
}

#[test]
fn synthesize_missing_source_exits_2() {
    let tmp = synth_tmp("missing");
    let out = Command::new(bin())
        .args(["synthesize", "-o"])
        .arg(tmp.join("out"))
        .arg(tmp.join("nope.sas"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no such file"));
}

#[test]
fn synthesize_rejects_csv_target_with_multiple_sources() {
    let tmp = synth_tmp("multi");
    let a = tmp.join("a.sas");
    std::fs::write(&a, "if x = 1 then y = 1;\n").unwrap();
    let out = Command::new(bin())
        .args(["synthesize", "-o"])
        .arg(tmp.join("out.csv"))
        .arg(&a)
        .arg(&a)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn synthesize_minimal_reports_branch_coverage() {
    let tmp = synth_tmp("minimal");
    let src = tmp.join("algo.sas");
    std::fs::write(
        &src,
        "if a = 1 then do;\n  if a = 2 then b = 1;\nend;\nelse if c in (3, 4) then d = 1;\n",
    )
    .unwrap();
    let target = tmp.join("min.csv");
    let out = Command::new(bin())
        .args(["synthesize", "--minimal", "-o"])
        .arg(&target)
        .arg(&src)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("(covers 5/6 branch outcomes)"));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("algo.sas:2: no row reaches the true branch")
    );
    let csv = std::fs::read_to_string(target).unwrap();
    assert_eq!(csv.lines().next(), Some("a,c"));
    assert_eq!(csv.lines().count(), 1 + 3);
}

#[test]
fn synthesize_rejects_bad_mix_options() {
    let tmp = synth_tmp("mix");
    let src = tmp.join("a.sas");
    std::fs::write(&src, "if x = 1 then y = 1;\n").unwrap();
    for bad in [
        &["--weights", "single=1,nope=2"][..],
        &["--perturbation-prob", "2"],
        &["--combo-size", "3-1"],
        &["--minimal", "--size", "10"],
    ] {
        let out = Command::new(bin())
            .arg("synthesize")
            .args(bad)
            .arg("-o")
            .arg(tmp.join("out.csv"))
            .arg(&src)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{bad:?}");
    }
}

#[test]
fn synthesize_minimal_adds_catalog_boundary_values() {
    let tmp = synth_tmp("boundaries");
    std::fs::write(
        tmp.join("catalog.csv"),
        "iCode;Acceptable Values\na;0-2\nb;0-3\nw;0.5-1.5\n",
    )
    .unwrap();
    let cfg = tmp.join("lint.yaml");
    std::fs::write(
        &cfg,
        format!(
            "rules:\n  variable_value_out_of_known_range:\n    enabled: false\n    \
             csv_paths: [{}]\n    name_column: iCode\n    delimiter: \";\"\n",
            tmp.join("catalog.csv").display()
        ),
    )
    .unwrap();
    let src = tmp.join("algo.sas");
    std::fs::write(
        &src,
        "if a in (0,1,2) and b in (0,1,2,3) then do;\n  \
         if a in (1,2) or b = 3 then c = 1;\nend;\nif w gt 1 then d = 1;\n",
    )
    .unwrap();
    let target = tmp.join("min.csv");
    let out = Command::new(bin())
        .args(["synthesize", "--minimal", "--config"])
        .arg(&cfg)
        .arg("-o")
        .arg(&target)
        .arg(&src)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout)
        .contains("(covers 6/6 branch outcomes, 12 boundary values)"));
    let csv = std::fs::read_to_string(target).unwrap();
    let mut lines = csv.lines();
    assert_eq!(lines.next(), Some("a,b,w"));
    let rows: Vec<Vec<String>> = lines
        .map(|l| l.split(',').map(String::from).collect())
        .collect();
    let column = |i: usize| -> Vec<&str> { rows.iter().map(|r| r[i].as_str()).collect() };
    for (i, edges) in [
        (0, &["-1", "0", "2", "3"][..]),
        (1, &["-1", "0", "3", "4"]),
        (2, &["0.4", "0.5", "1.5", "1.6"]),
    ] {
        for e in edges {
            assert!(
                column(i).contains(e),
                "column {i} lacks {e}: {:?}",
                column(i)
            );
        }
    }
    // Four distinct edges per input is the floor, and they pack into it.
    assert_eq!(rows.len(), 4);
}
