//! Focused suite for the `malformed_if_condition` rule: valid SAS syntax
//! the condition parser must accept without a finding.

use sas_linter::Linter;

const RULE: &str = "malformed_if_condition";

fn messages(src: &str) -> Vec<String> {
    let linter = Linter::from_ids(&[RULE.to_string()]).expect("rule id resolves");
    linter
        .lint(src, "example.sas")
        .into_iter()
        .map(|f| f.message)
        .collect()
}

#[test]
fn proc_sql_case_when_then_is_not_a_missing_if() {
    let src = "proc sql;\n\
               create table b as select id,\n\
               case when grp = 1 then 'low' when grp = 2 then 'mid' else 'high' end as band\n\
               from a;\n\
               quit;\n";
    assert_eq!(messages(src), Vec::<String>::new());
}

#[test]
fn nested_case_when_consumes_every_then() {
    let src = "proc sql;\n\
               select case when (case when a = 1 then 1 else 0 end) = 1 then 'y' else 'n' end\n\
               from t;\n\
               quit;\n";
    assert_eq!(messages(src), Vec::<String>::new());
}

#[test]
fn a_then_with_no_if_or_when_is_still_reported() {
    let src = "data b; set a;\n  x = 1 then y = 2;\nrun;\n";
    let m = messages(src);
    assert_eq!(m.len(), 1, "{m:?}");
    assert!(m[0].contains("without a preceding `if`"), "{}", m[0]);
}

#[test]
fn colon_modified_comparisons_are_operators() {
    for cond in [
        "name =: 'AB'",
        "name eq: 'AB'",
        "name ne: 'AB'",
        "name >=: 'M'",
        "name in: ('A', 'B')",
        "name not in: ('A', 'B')",
        "a = 1 and name =: 'X'",
    ] {
        let src = format!("data b; set a;\n  if {cond} then flag = 1;\nrun;\n");
        assert_eq!(messages(&src), Vec::<String>::new(), "{cond}");
    }
}

#[test]
fn special_missing_values_are_operands() {
    for cond in [
        "x = .a",
        "x = .Z",
        "x = ._",
        "x = .",
        "x in (.a, .b, .)",
        "x = .n or y = .d",
    ] {
        let src = format!("data b; set a;\n  if {cond} then flag = 1;\nrun;\n");
        assert_eq!(messages(&src), Vec::<String>::new(), "{cond}");
    }
}

#[test]
fn a_dot_followed_by_a_separate_word_is_still_a_missing_operator() {
    let src = "data b; set a;\n  if x = . a then flag = 1;\nrun;\n";
    assert_eq!(messages(src).len(), 1);
}
