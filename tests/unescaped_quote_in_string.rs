//! Focused suite for `unescaped_quote_in_string`: a quote inside a string
//! that matches its delimiter ends the string early (`'Children's …'`), and
//! the rest of the text runs on as code and a new string.

use sas_linter::Linter;

const RULE: &str = "unescaped_quote_in_string";

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

const RUNAWAY: &str = "label a = 'Children's score range';\nend;\nrun;\n";

#[test]
fn flags_an_apostrophe_that_ends_the_string() {
    let f = findings(RUNAWAY);
    assert_eq!(f.len(), 1, "{:?}", f);
    assert_eq!((f[0].line, f[0].column), (1, 20));
    assert!(f[0].message.contains("''"), "{}", f[0].message);
}

#[test]
fn flags_a_double_quote_in_a_double_quoted_string() {
    assert_eq!(findings("x = \"say \"hi\" now\";\n").len(), 1);
}

#[test]
fn typed_literals_and_keywords_after_a_string_are_fine() {
    let src = "d = '01jan2020'd; t = '09:00't; h = '09'x; 'my var'n = 1;\n\
               if name='a'then flag = 1;\n\
               x = 'It''s'; y = \"It's\";\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn autofix_doubles_the_quote() {
    assert_eq!(
        autofix(RUNAWAY),
        "label a = 'Children''s score range';\nend;\nrun;\n"
    );
}

#[test]
fn autofix_handles_several_apostrophes_in_one_string() {
    assert_eq!(
        autofix("x = 'Al's and Bo's';\n"),
        "x = 'Al''s and Bo''s';\n"
    );
}

#[test]
fn malformed_label_statement_defers_on_that_line() {
    let ids = vec![RULE.to_string(), "malformed_label_statement".to_string()];
    let f = Linter::from_ids(&ids).unwrap().lint(RUNAWAY, "t.sas");
    assert!(f.iter().all(|f| f.rule == RULE), "{:?}", f);
}

#[test]
fn a_string_that_ran_on_from_an_earlier_line_is_left_alone() {
    // Its real defect is wherever it opened; doubling here would extend it.
    let src = "x = 'broken\nmore text\n'Pain = 1;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
    assert_eq!(autofix(src), src);
}
