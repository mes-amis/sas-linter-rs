//! Focused integration test for `invalid_conventional_variable` — pattern +
//! CSV catalog combo doesn't fit the parametric loop in `fixtures_smoke.rs`.

use std::path::PathBuf;

use sas_linter::{Config, Linter};

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/lints/invalid_conventional_variable")
}

fn config_yaml(extra: &str) -> String {
    let csv_path = fixtures_root().join("catalog.csv");
    format!(
        r#"
rules:
  invalid_conventional_variable:
    pattern: '^[A-Z]+_\d+$'
    csv_paths:
      - {csv}
{extra}
"#,
        csv = csv_path.display(),
        extra = extra,
    )
}

fn linter(extra: &str) -> Linter {
    let cfg: Config = serde_yaml::from_str(&config_yaml(extra)).expect("config parses");
    Linter::from_config(&cfg).expect("linter builds")
}

fn findings_for(linter: &Linter, leaf: &str) -> Vec<sas_linter::Finding> {
    let path = fixtures_root().join(leaf);
    let src = std::fs::read_to_string(&path).unwrap();
    linter
        .lint(&src, &path.display().to_string())
        .into_iter()
        .filter(|f| f.rule == "invalid_conventional_variable")
        .collect()
}

#[test]
fn fires_on_unknown_pattern_match() {
    let findings = findings_for(&linter(""), "lint.sas");
    let messages: Vec<_> = findings.iter().map(|f| f.message.clone()).collect();
    assert!(
        findings.len() >= 3,
        "expected ≥3 findings, got {}: {:?}",
        findings.len(),
        messages,
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("ABC_1000") && m.contains("ABC_100")),
        "expected typo to suggest catalog form: {:?}",
        messages,
    );
    assert!(
        messages.iter().any(|m| m.contains("QRS_99")),
        "expected unrelated unknown to be flagged: {:?}",
        messages,
    );
}

#[test]
fn silent_on_clean_fixture() {
    let findings = findings_for(&linter(""), "clean.sas");
    assert!(
        findings.is_empty(),
        "expected silent, got: {:?}",
        findings.iter().map(|f| &f.message).collect::<Vec<_>>(),
    );
}

#[test]
fn allow_list_string_suppresses_match() {
    let extra = "    allow_list:\n      - QRS_99\n";
    let findings = findings_for(&linter(extra), "lint.sas");
    let messages: Vec<_> = findings.iter().map(|f| f.message.clone()).collect();
    assert!(
        !messages.iter().any(|m| m.contains("QRS_99")),
        "allow_list entry should suppress QRS_99: {:?}",
        messages,
    );
}

#[test]
fn allow_list_regex_suppresses_pattern() {
    let extra = "    allow_list:\n      - '/^QRS_\\d+$/'\n";
    let findings = findings_for(&linter(extra), "lint.sas");
    let messages: Vec<_> = findings.iter().map(|f| f.message.clone()).collect();
    assert!(
        !messages.iter().any(|m| m.contains("QRS_99")),
        "allow_list regex should suppress QRS_99: {:?}",
        messages,
    );
}

#[test]
fn no_op_without_pattern() {
    let cfg: Config = serde_yaml::from_str(
        r#"
rules:
  invalid_conventional_variable:
    csv_paths: []
"#,
    )
    .unwrap();
    let l = Linter::from_config(&cfg).unwrap();
    let path = fixtures_root().join("lint.sas");
    let src = std::fs::read_to_string(&path).unwrap();
    let findings: Vec<_> = l
        .lint(&src, &path.display().to_string())
        .into_iter()
        .filter(|f| f.rule == "invalid_conventional_variable")
        .collect();
    assert!(
        findings.is_empty(),
        "rule with no pattern should be a no-op: {:?}",
        findings.iter().map(|f| &f.message).collect::<Vec<_>>(),
    );
}
