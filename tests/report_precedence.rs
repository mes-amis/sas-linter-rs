//! When a specific rule explains a line, the general rules that trip over the
//! same line stay quiet (`rules::REPORT_PRECEDENCE`) — but only while the
//! specific rule is enabled, so the defect is never left unreported.

use sas_linter::Linter;

fn rules_on_line(ids: &[&str], src: &str, line: u32) -> Vec<&'static str> {
    let ids: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
    let mut rules: Vec<&'static str> = Linter::from_ids(&ids)
        .unwrap()
        .lint(src, "t.sas")
        .into_iter()
        .filter(|f| f.line == line)
        .map(|f| f.rule)
        .collect();
    rules.sort();
    rules.dedup();
    rules
}

/// Issue #1's example: the comment ends at the first `;`, and the rest of
/// the line is header text running as code.
const LEAKED_HEADER: &str =
    "**  REVISION DATES:   01/01/00; 02/02/01 (fixed typo: ... or x = 1 then do;)  **;\ndata a;\nrun;\n";

#[test]
fn leaked_header_text_is_reported_once_by_source_headers() {
    assert_eq!(
        rules_on_line(
            &[
                "source_headers",
                "statement_not_valid",
                "malformed_if_condition"
            ],
            LEAKED_HEADER,
            1
        ),
        vec!["source_headers"]
    );
}

#[test]
fn without_source_headers_the_general_rules_still_report_it() {
    assert!(!rules_on_line(&["statement_not_valid"], LEAKED_HEADER, 1).is_empty());
}

#[test]
fn a_multi_word_target_is_reported_by_invalid_assignment_target_alone() {
    let src = "data a;\nPredicted risk = 1;\nrun;\n";
    assert_eq!(
        rules_on_line(
            &["invalid_assignment_target", "statement_not_valid"],
            src,
            2
        ),
        vec!["invalid_assignment_target"]
    );
    // With invalid_assignment_target off, statement_not_valid reports it.
    assert_eq!(
        rules_on_line(&["statement_not_valid"], src, 2),
        vec!["statement_not_valid"]
    );
}

#[test]
fn precedence_only_names_real_rules() {
    let metas = sas_linter::rules::all_metas();
    for (winner, losers) in sas_linter::rules::REPORT_PRECEDENCE {
        for id in std::iter::once(winner).chain(losers.iter()) {
            assert!(metas.iter().any(|m| m.id == *id), "unknown rule {}", id);
        }
    }
}
