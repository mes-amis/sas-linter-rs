//! Focused integration test for `variable_value_out_of_known_range` —
//! the rule depends on a CSV catalog so it doesn't fit the parametric
//! fixture loop in `fixtures_smoke.rs`.

use std::path::PathBuf;

use sas_linter::{Config, Linter};

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/lints/variable_value_out_of_known_range")
}

fn config_yaml() -> String {
    let csv_path = fixtures_root().join("variables.csv");
    format!(
        r#"
rules:
  variable_value_out_of_known_range:
    csv_paths:
      - {csv}
    delimiter: ";"
"#,
        csv = csv_path.display(),
    )
}

fn linter() -> Linter {
    let cfg: Config = serde_yaml::from_str(&config_yaml()).expect("config parses");
    Linter::from_config(&cfg).expect("linter builds")
}

#[test]
fn fires_on_out_of_range_literals() {
    let path = fixtures_root().join("lint.sas");
    let src = std::fs::read_to_string(&path).unwrap();
    let findings: Vec<_> = linter()
        .lint(&src, &path.display().to_string())
        .into_iter()
        .filter(|f| f.rule == "variable_value_out_of_known_range")
        .collect();
    let messages: Vec<_> = findings.iter().map(|f| f.message.clone()).collect();
    assert!(
        findings.len() >= 4,
        "expected ≥4 findings, got {}: {:?}",
        findings.len(),
        messages
    );
    // Verify each motivating violation is flagged.
    assert!(messages
        .iter()
        .any(|m| m.contains("99") && m.contains("V1")));
    assert!(messages
        .iter()
        .any(|m| m.contains("99") && m.contains("SCORE")));
    assert!(messages
        .iter()
        .any(|m| m.contains("7") && m.contains("RANK")));
    assert!(messages.iter().any(|m| m.contains("9") && m.contains("V4")));
}

#[test]
fn silent_on_clean_fixture() {
    let path = fixtures_root().join("clean.sas");
    let src = std::fs::read_to_string(&path).unwrap();
    let findings: Vec<_> = linter()
        .lint(&src, &path.display().to_string())
        .into_iter()
        .filter(|f| f.rule == "variable_value_out_of_known_range")
        .collect();
    assert!(
        findings.is_empty(),
        "expected silent, got: {:?}",
        findings.iter().map(|f| &f.message).collect::<Vec<_>>()
    );
}
