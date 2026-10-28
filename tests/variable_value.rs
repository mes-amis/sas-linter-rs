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

fn reported(src: &str) -> Vec<String> {
    linter()
        .lint(src, "example.sas")
        .into_iter()
        .filter(|f| f.rule == "variable_value_out_of_known_range")
        .map(|f| format!("{}:{}", f.line, f.column))
        .collect()
}

#[test]
fn only_a_whole_comparison_is_checked() {
    // `V1 = 9` is part of a larger expression in each of these, not a test
    // of V1 against 9.
    for cond in [
        "SCORE + V1 = 9",
        "V1 = 9 + 1",
        "V1 = 9 * SCORE",
        "V1 in (9) + 1",
        "not V1 = 9",
    ] {
        let src = format!("if {cond} then cOut = 1;\n");
        assert_eq!(reported(&src), Vec::<String>::new(), "{cond}");
    }
}

#[test]
fn each_and_or_operand_is_checked() {
    let src = "if V1 = 9 and SCORE in (1, 7) then cOut = 1;\n\
               if (RANK = 1 | V1 = 9) & (V4 = 4) then cOut = 2;\n\
               if V1 = -1 then cOut = 3;\n";
    assert_eq!(reported(src), vec!["1:9", "1:28", "2:21", "2:32", "3:9"]);
}

#[test]
fn missing_values_in_a_list_are_not_checked_but_do_not_hide_the_rest() {
    let src = "if V1 in (., .a, ._, 9) then cOut = 1;\n";
    assert_eq!(reported(src), vec!["1:22"]);
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
