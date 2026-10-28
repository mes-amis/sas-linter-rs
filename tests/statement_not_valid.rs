//! Focused suite for `statement_not_valid`: a statement that can't start
//! the way it does — bare text, stray punctuation — which SAS rejects with
//! `ERROR 180-322`.

use sas_linter::Linter;

const RULE: &str = "statement_not_valid";

fn findings(src: &str) -> Vec<sas_linter::Finding> {
    let linter = Linter::from_ids(&[RULE.to_string()]).expect("rule id resolves");
    linter.lint(src, "example.sas")
}

fn positions(src: &str) -> Vec<(u32, u32)> {
    findings(src).iter().map(|f| (f.line, f.column)).collect()
}

#[test]
fn flags_bare_text_before_the_header() {
    let src = "my_program_name_v1.0.txt\n** Program title **;\ndata foo;\nrun;\n";
    let f = findings(src);
    assert_eq!(positions(src), vec![(1, 1)]);
    assert!(
        f[0].message.contains("my_program_name_v1.0.txt"),
        "{}",
        f[0].message
    );
    assert!(f[0].message.contains("ERROR 180-322"), "{}", f[0].message);
}

#[test]
fn flags_a_stray_dot_after_a_block_comment() {
    let src = "data foo;\n/*\n spss code\n*/.\nx = 1;\nrun;\n";
    assert_eq!(positions(src), vec![(4, 3)]);
}

#[test]
fn flags_bare_words_and_literals_in_a_data_step() {
    let src = "data foo;\n\
               Compute the score;\n\
               5 = x;\n\
               'text';\n\
               run;\n";
    assert_eq!(positions(src), vec![(2, 1), (3, 1), (4, 1)]);
}

#[test]
fn proc_bodies_are_not_checked() {
    // PROC statements vary per PROC; only data steps and open code are.
    let src = "proc format;\n value x 0 = 'a';\n 1 = 'b';\nrun;\n\
               proc reg data=a;\n model y = x / noint;\nrun;\nquit;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn checking_resumes_after_a_proc() {
    let src = "proc print;\nrun;\ndata foo;\nstray words;\nrun;\n";
    assert_eq!(positions(src), vec![(4, 1)]);
}

#[test]
fn run_ends_a_proc_so_open_code_after_it_is_checked() {
    let src = "proc format;\n value x 0 = 'a';\nrun;\n/* old code */.\n";
    assert_eq!(positions(src), vec![(4, 15)]);
}

#[test]
fn run_group_procs_stay_unchecked_until_quit() {
    // PROC DATASETS / REG / SQL / … keep accepting statements after `run;`.
    let src = "proc datasets lib=work;\n modify foo;\n rename a = b;\nrun;\n\
               change foo = bar;\nrun;\nquit;\n\
               proc reg data=a;\n model y = x;\nrun;\n model y = z;\nrun;\nquit;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn a_multi_word_assignment_target_is_left_to_invalid_assignment_target() {
    let src = "data foo;\n Predicted risk = 1;\nrun;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn statements_with_macro_code_are_not_checked() {
    let src = "data foo;\n&var = 1;\n%if &x %then %do; y = 1; %end;\nrun;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn valid_statement_shapes_are_silent() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/lints/statement_not_valid/clean.sas"
    ))
    .unwrap();
    assert!(findings(&src).is_empty(), "{:?}", findings(&src));
}

#[test]
fn rule_is_report_only() {
    let meta = sas_linter::rules::all_metas();
    let m = meta.iter().find(|m| m.id == RULE).expect("rule registered");
    assert!(!m.supports_autofix);
}
