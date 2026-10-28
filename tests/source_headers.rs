//! Focused suite for `source_headers`, driven by the `source_headers_autofix`
//! fixtures: header prose that leaks out of its `**` comment into code is
//! re-wrapped; everything SAS already reads correctly is left alone.

use std::path::PathBuf;

use sas_linter::Linter;

const RULE: &str = "source_headers";

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/source_headers_autofix")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

fn findings(src: &str) -> Vec<sas_linter::Finding> {
    Linter::from_ids(&[RULE.to_string()])
        .unwrap()
        .lint(src, "t.sas")
}

fn autofix(src: &str) -> String {
    let meta = sas_linter::rules::all_metas();
    let m = meta.iter().find(|m| m.id == RULE).expect("rule registered");
    let opts: serde_yaml::Value = serde_yaml::from_str("autofix: true").unwrap();
    (m.config_factory)(&opts).unwrap().autofix(src)
}

/// Findings from the rules that see code where a comment was meant.
fn leaks(src: &str) -> Vec<sas_linter::Finding> {
    Linter::from_ids(&[
        "statement_not_valid".to_string(),
        "unterminated_comment".to_string(),
        RULE.to_string(),
    ])
    .unwrap()
    .lint(src, "t.sas")
}

#[test]
fn sources_sas_reads_correctly_are_left_alone() {
    for name in [
        "already_correct.sas",
        "body_code_no_data_step.sas",
        "body_inline_comment.sas",
        "body_marker_between_code.sas",
        "chained_two_comments_one_line.sas",
        "narrative_comment_no_data_step.sas",
        "narrative_multiline_comment.sas",
        "null_statement_after_comment.sas",
        "tabs_in_header.sas",
    ] {
        let src = fixture(name);
        assert!(findings(&src).is_empty(), "{}: {:?}", name, findings(&src));
        assert_eq!(autofix(&src), src, "{} must not change", name);
    }
}

#[test]
fn broken_headers_are_reported_and_repaired() {
    for name in [
        "continuation_line.sas",
        "inline_semi_in_header.sas",
        "missing_terminators.sas",
    ] {
        let src = fixture(name);
        assert!(!findings(&src).is_empty(), "{} should be reported", name);
        let fixed = autofix(&src);
        assert!(
            leaks(&fixed).is_empty(),
            "{} still leaks: {:?}\n{}",
            name,
            leaks(&fixed),
            fixed
        );
        assert!(
            fixed.contains("data one; set have;"),
            "{} lost its code:\n{}",
            name,
            fixed
        );
    }
}

#[test]
fn a_semicolon_in_header_text_becomes_a_comma() {
    let src = "**  REVISION DATES:   01/02/2003; 04/05/2006  **;\ndata a;\nrun;\n";
    assert_eq!(findings(src).len(), 1, "{:?}", findings(src));
    let fixed = autofix(src);
    assert!(
        fixed.starts_with("**  REVISION DATES:   01/02/2003, 04/05/2006"),
        "{}",
        fixed
    );
    assert!(leaks(&fixed).is_empty(), "{:?}", leaks(&fixed));
}

#[test]
fn code_after_a_comment_on_the_same_line_is_left_alone() {
    let src = "data a;\n** reset counter; count = 0;\nrun;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
    assert_eq!(autofix(src), src);
}

#[test]
fn an_exponent_on_a_continuation_line_is_left_alone() {
    let src = "data a;\ny = base\n ** power;\nrun;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
    assert_eq!(autofix(src), src);
}

#[test]
fn a_box_whose_top_row_lacks_a_semicolon_is_one_valid_comment() {
    let src = "*****************************************\n\
               ** PROGRAM:  scores                    **\n\
               *****************************************;\n\
               data a;\nrun;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
    assert_eq!(autofix(src), src);
}

#[test]
fn tabs_outside_rewritten_lines_are_left_alone() {
    let src = "**  CHECKED BY:  A Person; B Person\ndata a;\n  x = scan(s, 1, \"\t\");\nrun;\n";
    let fixed = autofix(src);
    assert!(fixed.contains("scan(s, 1, \"\t\")"), "{}", fixed);
}
