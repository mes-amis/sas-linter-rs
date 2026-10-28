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

#[test]
fn numbered_ranges_cover_every_member() {
    // An array names its elements, and `a{i}` then uses them; a numbered
    // range in either place covers `a2` as well as its endpoints.
    let via_array = "data foo;\n array a{3} a1-a3;\n do i = 1 to 3; a{i} = 0; end;\n \
                     format a1-a3 8.;\nrun;\n";
    assert!(findings(via_array).is_empty(), "{:?}", findings(via_array));

    let via_of_list = "data foo;\n total = sum(of q1-q3);\n format q2 8.;\nrun;\n";
    assert!(
        findings(via_of_list).is_empty(),
        "{:?}",
        findings(via_of_list)
    );
}

#[test]
fn a_numbered_range_is_flagged_once_when_no_member_is_used() {
    let src = "data foo;\n x = 1;\n format r1-r3 8.;\nrun;\n";
    let f = findings(src);
    assert_eq!(f.len(), 1, "{:?}", f);
    assert_eq!((f[0].line, f[0].column), (3, 9));
    assert!(f[0].message.contains("`r1-r3`"), "{:?}", f[0]);
}

#[test]
fn name_and_positional_ranges_reference_their_endpoints() {
    // `b--d` / `b-numeric-d` span variables between the endpoints; only the
    // endpoints are names, and `numeric` is not a variable at all.
    let src = "data foo;\n b = 1; d = 2;\n total = sum(of b--d);\n \
               format b--d 8.;\n format b-numeric-d 8.;\nrun;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn conditional_reads_count_as_external_input() {
    // `set` / `input` needn't start the statement to read variables in.
    for read in [
        "if _n_ = 1 then set lookup;",
        "if x then y = 1; else set other;",
        "next: input a b;",
    ] {
        let src = format!("data foo;\n {read}\n label anything = 'x';\nrun;\n");
        assert!(findings(&src).is_empty(), "{read}: {:?}", findings(&src));
    }
}

#[test]
fn proc_statements_are_not_external_input() {
    // PROC SQL's `update … set` reads nothing into a data step.
    let src = "proc sql;\n update t set a = 1;\nquit;\n\
               data foo;\n x = 1;\n label typo = 'T';\nrun;\n";
    let f = findings(src);
    assert_eq!(f.len(), 1, "{:?}", f);
    assert_eq!(f[0].line, 6);
}

#[test]
fn a_proc_ends_at_run_or_its_last_statement() {
    // Fragments follow a PROC with the data-step body and no `data`
    // statement: after `run;`, or straight after a `run;`-less PROC FORMAT.
    for proc in [
        "proc sort data = a;\n by k;\nrun;\n",
        "proc format;\n value f 0 = 'a';\n",
    ] {
        let src = format!("{proc}x = 1;\nformat typo f.;\n");
        let f = findings(&src);
        assert_eq!(f.len(), 1, "{proc}: {:?}", f);
        assert!(f[0].message.contains("`typo`"), "{:?}", f[0]);
    }
}
