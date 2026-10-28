//! Focused suite for `orphan_format_range`: a stray `;` inside a PROC
//! FORMAT `value` statement leaves the remaining ranges as statements of
//! their own (`ERROR 180-322`).

use sas_linter::Linter;

const RULE: &str = "orphan_format_range";

fn findings(src: &str) -> Vec<sas_linter::Finding> {
    let linter = Linter::from_ids(&[RULE.to_string()]).expect("rule id resolves");
    linter.lint(src, "example.sas")
}

fn positions(src: &str) -> Vec<(u32, u32)> {
    findings(src).iter().map(|f| (f.line, f.column)).collect()
}

fn autofix(src: &str) -> String {
    let meta = sas_linter::rules::all_metas();
    let m = meta.iter().find(|m| m.id == RULE).expect("rule registered");
    let opts: serde_yaml::Value = serde_yaml::from_str("autofix: true").unwrap();
    (m.config_factory)(&opts).unwrap().autofix(src)
}

const ISSUE: &str = "proc format;\n\
                     \x20  value levels 0 = 'Low'\n\
                     \x20               1 = 'Medium';\n\
                     \x20               2 = 'High';\n\
                     run;\n";

#[test]
fn flags_a_range_after_a_value_statement() {
    let f = findings(ISSUE);
    assert_eq!(positions(ISSUE), vec![(4, 17)]);
    assert!(f[0].message.contains("`value levels`"), "{}", f[0].message);
}

#[test]
fn flags_every_orphan_in_a_chain() {
    let src = "proc format;\n value x 0 = 'a'; 1 = 'b'; 2 = 'c';\nrun;\n";
    assert_eq!(positions(src), vec![(2, 19), (2, 28)]);
}

#[test]
fn flags_strings_ranges_and_keywords() {
    for orphan in [
        "'b' = 'B'",
        "1 - 5 = 'low'",
        "low - 0 = 'neg'",
        "other = 'x'",
        "-1 = 'm'",
    ] {
        let src = format!("proc format;\n invalue $x 'a' = 'A'; {};\nrun;\n", orphan);
        assert_eq!(findings(&src).len(), 1, "{}", orphan);
    }
    let src = "proc format;\n picture p low-0 = '9'; 1-high = '0009';\nrun;\n";
    assert_eq!(findings(src).len(), 1);
}

#[test]
fn ignores_code_outside_proc_format() {
    let src = "proc format;\n value x 0 = 'a';\nrun;\n\
               data foo;\n value x 0 = 1;\n 1 = 2;\nrun;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn autofix_removes_the_early_semicolon() {
    assert_eq!(
        autofix(ISSUE),
        "proc format;\n\
         \x20  value levels 0 = 'Low'\n\
         \x20               1 = 'Medium'\n\
         \x20               2 = 'High';\n\
         run;\n"
    );
    assert_eq!(
        autofix("proc format;\n value x 0 = 'a'; 1 = 'b'; 2 = 'c';\nrun;\n"),
        "proc format;\n value x 0 = 'a' 1 = 'b' 2 = 'c';\nrun;\n"
    );
}
