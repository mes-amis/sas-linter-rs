//! Focused suite for `malformed_label_statement`: the missing-`=` check the
//! fixtures cover, plus a `label` statement whose `;` is missing so it
//! swallows the next statement.

use sas_linter::Linter;

const RULE: &str = "malformed_label_statement";

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
fn flags_a_label_that_swallows_an_attrib() {
    let src = "label  cOUT1 = 'Output one (0,1)'\n\
               attrib cOUT1 format = cout.;\n";
    let f = findings(src);
    assert_eq!(f.len(), 1, "{:?}", f);
    assert_eq!((f[0].line, f[0].column), (2, 1));
    assert!(
        f[0].message.contains("missing its terminating `;`"),
        "{}",
        f[0].message
    );
    assert!(f[0].message.contains("`attrib`"), "{}", f[0].message);
}

#[test]
fn flags_a_label_that_swallows_another_label() {
    let src = "label  cOUT1 = 'Output one (0,1)'\n\
               label  cOUT2 = 'Output two (0,1)';\n";
    let f = findings(src);
    assert_eq!(f.len(), 1, "{:?}", f);
    assert_eq!((f[0].line, f[0].column), (2, 1));
}

#[test]
fn flags_each_statement_keyword() {
    for kw in [
        "label", "attrib", "format", "informat", "length", "if", "else", "do", "end", "drop",
        "keep", "run",
    ] {
        let src = format!("label a = 'A'\n{} b;\n", kw);
        assert_eq!(findings(&src).len(), 1, "keyword {}", kw);
    }
}

#[test]
fn a_keyword_named_variable_with_its_own_label_is_not_flagged() {
    // `length` here is a variable being labelled, not a new statement.
    let src = "label a = 'A' length = 'Length of stay' format='Format code';\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn a_terminated_label_is_silent() {
    let src = "label  cOUT1 = 'Output one (0,1)';\n\
               attrib cOUT1 format = cout.;\n\
               label  a = 'A' b = 'B';\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn autofix_inserts_the_semicolon_after_the_string() {
    let src = "label  cOUT1 = 'Output one (0,1)'\n\
               attrib cOUT1 format = cout.;\n\
               label  a = 'A' label b = 'B';\n";
    assert_eq!(
        autofix(src),
        "label  cOUT1 = 'Output one (0,1)';\n\
         attrib cOUT1 format = cout.;\n\
         label  a = 'A'; label b = 'B';\n"
    );
}

#[test]
fn autofix_fixes_both_defects_together() {
    let src = "label a 'A'\nrun;\n";
    assert_eq!(autofix(src), "label a = 'A';\nrun;\n");
}

#[test]
fn label_options_of_other_statements_are_not_label_statements() {
    // SAS/GRAPH `label=(…)` options and ATTRIB's `label=` are options, not
    // `label` statements.
    let src = "axis1 label=(f=swiss j=c 'Weight') order=(0 to 10);\n\
               attrib x label='Lbl' length=8;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
    assert_eq!(autofix(src), src);
}

#[test]
fn the_message_quotes_only_the_first_line_of_a_long_string() {
    let src = "label a 'first line\nsecond line';\n";
    let f = findings(src);
    assert_eq!(f.len(), 1, "{:?}", f);
    assert!(!f[0].message.contains('\n'), "{:?}", f[0].message);
}
