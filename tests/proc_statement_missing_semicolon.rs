//! Focused suite for `proc_statement_missing_semicolon`: a PROC statement
//! with no `;` absorbs the statement after it as PROC options.

use sas_linter::Linter;

const RULE: &str = "proc_statement_missing_semicolon";

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
fn flags_proc_format_swallowing_a_value_statement() {
    let src = "proc format\n   value fmtname 0 = 'No'\n   1 = 'Yes';\nrun;\n";
    let f = findings(src);
    assert_eq!(f.len(), 1, "{:?}", f);
    assert_eq!((f[0].line, f[0].column), (1, 1));
    assert!(f[0].message.contains("`proc format`"), "{}", f[0].message);
    assert!(f[0].message.contains("`value`"), "{}", f[0].message);
}

#[test]
fn flags_invalue_and_picture_too() {
    for kw in ["invalue", "picture", "VALUE"] {
        let src = format!("proc format lib=work\n {} x 1 = 'a';\nrun;\n", kw);
        assert_eq!(findings(&src).len(), 1, "keyword {}", kw);
    }
}

#[test]
fn flags_other_procs_absorbing_their_first_statement() {
    for src in [
        "proc sort data=a\n by id;\nrun;\n",
        "proc freq data=a\n tables x*y;\nrun;\n",
        "proc means data=a\n var x;\nrun;\n",
        "proc print data=a\n var x;\nrun;\n",
    ] {
        assert_eq!(findings(src).len(), 1, "{}", src);
    }
}

#[test]
fn a_keyword_inside_an_option_value_is_silent() {
    let src = "proc print data=a(keep=var value);\n var x;\nrun;\n\
               proc format cntlin=value;\nrun;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn terminated_proc_statements_are_silent() {
    let src = "proc format;\n value x 1 = 'a';\nrun;\n\
               proc sort data=a;\n by id;\nrun;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn autofix_terminates_the_proc_statement() {
    assert_eq!(
        autofix("proc format\n   value fmtname 0 = 'No';\nrun;\n"),
        "proc format;\n   value fmtname 0 = 'No';\nrun;\n"
    );
    assert_eq!(
        autofix("proc sort data=a out=b\n by id;\nrun;\n"),
        "proc sort data=a out=b;\n by id;\nrun;\n"
    );
}
