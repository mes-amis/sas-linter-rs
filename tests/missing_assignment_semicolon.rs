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
