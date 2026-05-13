//! Parametric integration tests that pair each rule with its
//! `lint.sas` / `clean.sas` fixture. Mirrors the RSpec suite's
//! convention: lint fixtures must produce ≥1 finding from the rule
//! under test; clean fixtures must be silent.
//!
//! Fixture subdir → rule id mapping. Most are 1:1; the two prefixed
//! exceptions (`unreachable_inner`, `unreachable_inner_eq`) both map
//! to `unreachable_inner_branch_value`, and `malformed_if_condition_cascade`
//! piggybacks on `malformed_if_condition`.

use std::path::PathBuf;

use sas_linter::Linter;

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lints")
}

fn lint_with(rule_id: &str, fixture: &str, leaf: &str) -> Vec<sas_linter::Finding> {
    let path = fixtures_root().join(fixture).join(leaf);
    let linter = Linter::from_ids(&[rule_id.to_string()]).expect("rule id resolves");
    let src = std::fs::read_to_string(&path).expect("fixture readable");
    linter.lint(&src, &path.display().to_string())
}

/// Fixture dirs paired with the rule id they exercise. Order doesn't
/// matter — each pair runs both `lint.sas` (must fire) and `clean.sas`
/// (must be silent).
const PAIRS: &[(&str, &str)] = &[
    ("trailing_whitespace", "trailing_whitespace"),
    ("tab_expansion", "tab_expansion"),
    ("malformed_label_statement", "malformed_label_statement"),
    ("identical_if_else_branches", "identical_if_else_branches"),
    ("malformed_if_condition", "malformed_if_condition"),
    ("malformed_if_condition", "malformed_if_condition_cascade"),
    (
        "missing_assignment_semicolon",
        "missing_assignment_semicolon",
    ),
    ("invalid_numeric_literal", "invalid_numeric_literal"),
    ("commented_out_guard", "commented_out_guard"),
    ("choose_one_template", "choose_one_template"),
    ("unreachable_inner_branch_value", "unreachable_inner"),
    ("unreachable_inner_branch_value", "unreachable_inner_eq"),
    ("format_for_unknown_variable", "format_for_unknown_variable"),
    ("inconsistent_variable_case", "inconsistent_variable_case"),
    ("unterminated_comment", "unterminated_comment"),
    ("source_headers", "source_headers"),
];

#[test]
fn lint_fixtures_produce_findings() {
    let mut failures = Vec::new();
    for (rule, dir) in PAIRS {
        let findings = lint_with(rule, dir, "lint.sas");
        if findings.is_empty() {
            failures.push(format!(
                "rule {} on {}/lint.sas produced no findings",
                rule, dir
            ));
        } else if !findings.iter().any(|f| f.rule == *rule) {
            failures.push(format!(
                "rule {} on {}/lint.sas produced findings but none from this rule: {:?}",
                rule,
                dir,
                findings.iter().map(|f| f.rule).collect::<Vec<_>>()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn clean_fixtures_are_silent() {
    let mut failures = Vec::new();
    for (rule, dir) in PAIRS {
        let path = fixtures_root().join(dir).join("clean.sas");
        if !path.exists() {
            // Not every rule has a clean.sas — only assert the ones
            // that ship one. Mirrors the RSpec suite, which skips
            // pairs that don't have a paired clean fixture.
            continue;
        }
        let findings = lint_with(rule, dir, "clean.sas");
        if !findings.is_empty() {
            failures.push(format!(
                "rule {} on {}/clean.sas should be silent but produced {} findings: {}",
                rule,
                dir,
                findings.len(),
                findings
                    .iter()
                    .map(|f| format!("{}:{}:{} {}", f.line, f.column, f.rule, f.message))
                    .collect::<Vec<_>>()
                    .join("\n  ")
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
