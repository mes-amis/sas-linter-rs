//! Focused suite for `format_for_unknown_variable` beyond the fixture pair:
//! `label` statements name variables the same way `format` does.

use sas_linter::Linter;

const RULE: &str = "format_for_unknown_variable";

fn findings(src: &str) -> Vec<sas_linter::Finding> {
    let linter = Linter::from_ids(&[RULE.to_string()]).expect("rule id resolves");
    linter.lint(src, "example.sas")
}

#[test]
fn flags_a_label_for_an_unknown_variable() {
    let src = "data foo;\n\
               if x = 1 then aOUT = 1;\n\
               label saOUT = 'Output description';\n\
               run;\n";
    let f = findings(src);
    assert_eq!(f.len(), 1, "{:?}", f);
    assert_eq!((f[0].line, f[0].column), (3, 7));
    assert!(f[0].message.contains("`label`"), "{}", f[0].message);
    assert!(f[0].message.contains("`saOUT`"), "{}", f[0].message);
}

#[test]
fn flags_each_unknown_variable_in_one_label_statement() {
    let src = "data foo;\n\
               a = 1;\n\
               label a = 'A' typo1 = 'B' typo2 'C';\n\
               run;\n";
    let names: Vec<String> = findings(src)
        .iter()
        .map(|f| f.message.split('`').nth(3).unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["typo1", "typo2"]);
}

#[test]
fn a_label_for_a_known_variable_is_silent() {
    let src = "data foo;\n\
               if x = 1 then aOUT = 1;\n\
               label aout = 'Output description';\n\
               attrib aOUT label = 'Again' format = best.;\n\
               run;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn labels_are_skipped_when_the_step_reads_external_input() {
    let src = "data foo;\n set bar;\n label anything = 'x';\nrun;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn proc_steps_are_skipped() {
    // PROC statements name variables of the input dataset (`data=foo`),
    // which this file never mentions otherwise.
    let src = "proc print data=foo label;\n\
               label score = 'Score';\n\
               format score best.;\n\
               run;\n\
               data bar;\n\
               x = 1;\n\
               label typo = 'T';\n\
               run;\n";
    let f = findings(src);
    assert_eq!(f.len(), 1, "{:?}", f);
    assert_eq!(f[0].line, 7);
}
