//! Focused suite for `missing_assignment_semicolon`: an assignment whose
//! `;` is missing, so the inline comment that follows is parsed as part of
//! the expression — `**` as exponentiation, `*` as multiplication.

use sas_linter::Linter;

const RULE: &str = "missing_assignment_semicolon";

fn findings(src: &str) -> Vec<sas_linter::Finding> {
    let linter = Linter::from_ids(&[RULE.to_string()]).expect("rule id resolves");
    linter.lint(src, "example.sas")
}

fn autofix(src: &str) -> String {
    let meta = sas_linter::rules::all_metas();
    let m = meta.iter().find(|m| m.id == RULE).expect("rule registered");
    let opts: serde_yaml::Value = serde_yaml::from_str("autofix: true").unwrap();
    (m.config_factory)(&opts).unwrap().autofix(src)
}

#[test]
fn flags_a_double_star_comment() {
    let src = "x = y  ** Some description;\n";
    assert_eq!(findings(src).len(), 1);
}

#[test]
fn flags_a_single_star_comment() {
    let src = "x = x * Some description of x;\n";
    let f = findings(src);
    assert_eq!(f.len(), 1, "{:?}", f);
    assert_eq!((f[0].line, f[0].column), (1, 7));
    assert!(f[0].message.contains("`*`"), "{}", f[0].message);
}

#[test]
fn flags_a_single_star_comment_after_then() {
    let src = "if a = 1 then x = 2 * set x to two;\n";
    assert_eq!(findings(src).len(), 1, "{:?}", findings(src));
}

#[test]
fn ordinary_multiplication_is_silent() {
    let src = "x = x * y;\n\
               x = a * b + c * (d - e);\n\
               y = y;   * Another description;\n\
               z = 2 * w;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn star_outside_an_assignment_is_silent() {
    // PROC FREQ crossings put bare words after `*` legitimately.
    let src = "proc freq;\n tables a * b c;\nrun;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn autofix_terminates_the_assignment_before_the_star() {
    let src = "x = x * Some description of x;\ny = 1;\n";
    assert_eq!(autofix(src), "x = x; * Some description of x;\ny = 1;\n");
}

#[test]
fn exponentiation_is_silent() {
    let src = "area = radius ** two;\n\
               y = base**power + 1;\n\
               z = (a + b) ** 2;\n\
               w = base\n   ** power;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
    assert_eq!(autofix(src), src);
}

#[test]
fn flags_a_double_star_comment_ending_in_punctuation() {
    let src = "iA = iA      **   Mobility device: cane;\n";
    assert_eq!(findings(src).len(), 1, "{:?}", findings(src));
}

#[test]
fn autofix_keeps_the_comment_column() {
    // With room to spare, the `;` takes one of the blanks before the comment.
    assert_eq!(
        autofix("Q1 = Q1     **  Some note;\nX = X ** Other note;\n"),
        "Q1 = Q1;    **  Some note;\nX = X; ** Other note;\n"
    );
}

#[test]
fn prose_with_operator_words_is_still_prose() {
    // `to` is no operator in an expression; `or` is, but `unusual reaction`
    // are two operands side by side.
    let src = "a = a  **  Ability to adapt in routine or setting;\n\
               b = b  **  Excessive or unusual reaction to noise;\n";
    assert_eq!(findings(src).len(), 2, "{:?}", findings(src));
}

#[test]
fn the_comment_starts_at_the_last_marker_before_the_prose() {
    let src = "x = a * b ** Note on x;\n";
    let f = findings(src);
    assert_eq!(f.len(), 1, "{:?}", f);
    assert_eq!(f[0].column, 11, "{:?}", f);
    assert_eq!(autofix(src), "x = a * b; ** Note on x;\n");
}

#[test]
fn a_comment_line_swallowed_by_an_unterminated_statement() {
    // The `value` statement lacks its `;`, so the banner comment on the
    // next line is parsed as more of it.
    let src = "proc format;\n   value cap 0 = 'No'\n             1 = 'Yes'\n\n\
               **  DATA STEP STARTS HERE  **;\n\ndata a;\nrun;\n";
    let f = findings(src);
    assert_eq!(f.len(), 1, "{:?}", f);
    assert_eq!((f[0].line, f[0].column), (5, 1));
    assert_eq!(
        autofix(src),
        "proc format;\n   value cap 0 = 'No'\n             1 = 'Yes';\n\n\
         **  DATA STEP STARTS HERE  **;\n\ndata a;\nrun;\n"
    );
}

#[test]
fn a_mid_line_star_outside_an_assignment_stays_silent() {
    let src = "proc freq;\n tables a * b c;\nrun;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn leaked_header_prose_is_not_a_statement_missing_its_semicolon() {
    // The box has no terminators, so its comment ends at `A Person;` and the
    // rest is header prose running as code: source_headers' finding.
    let src = "*****************************************\n\
               **  CHECKED BY:  A Person; B Person\n\
               **  DATE:        01/02/2003\n\
               **  PURPOSE:     compute summary scores\n\
               *****************************************;\n\
               data a;\nrun;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn one_finding_per_statement() {
    let src = "x = y\n** first note line\n** second note line;\n";
    assert_eq!(findings(src).len(), 1, "{:?}", findings(src));
}
