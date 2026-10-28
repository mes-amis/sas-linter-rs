//! Focused suite for the `inconsistent_variable_case` rule.
//!
//! Only identifiers in *variable* positions of a data step count. Dataset
//! names, function and CALL-routine names, statement keywords the lexer
//! leaves as plain identifiers, statement labels, and anything inside a PROC
//! step are other namespaces (or not names at all) — rewriting their case
//! "for consistency" is noise at best.

use sas_linter::Linter;

const RULE: &str = "inconsistent_variable_case";

fn flagged(src: &str) -> Vec<String> {
    let linter = Linter::from_ids(&[RULE.to_string()]).expect("rule id resolves");
    let mut out: Vec<String> = linter
        .lint(src, "test.sas")
        .iter()
        .map(|f| f.message.split('`').nth(1).unwrap_or_default().to_string())
        .collect();
    out.sort();
    out
}

fn autofixed(src: &str) -> String {
    let cfg: sas_linter::Config =
        serde_yaml::from_str(&format!("rules:\n  {RULE}:\n    autofix: true\n")).unwrap();
    let rules = sas_linter::rules::rules_from_config(&cfg).unwrap();
    let rule = rules.iter().find(|r| r.id() == RULE).unwrap();
    rule.autofix(src)
}

#[test]
fn dataset_names_are_not_variables() {
    let src = "data Out1;\n  set out1 Work.In1;\n  merge in1 IN2;\n  update in2 In3;\n  \
               if x then output OUT1;\nrun;\n";
    assert!(flagged(src).is_empty(), "{:?}", flagged(src));
    assert_eq!(autofixed(src), src);
}

#[test]
fn function_and_call_routine_names_are_not_variables() {
    let src = "a = SUBSTR(b, 1, 1);\nc = substr(d, 1, 1);\n\
               call SYMPUTX('m', a);\ncall symputx('n', c);\n";
    assert!(flagged(src).is_empty(), "{:?}", flagged(src));
}

#[test]
fn a_declared_array_reference_is_a_variable_use() {
    // `(` after a name means a function call — unless the name is an array.
    let src = "array Scores(3) s1-s3;\ntotal = scores(1) + Scores(2);\n";
    assert_eq!(flagged(src), vec!["scores"]);
    assert_eq!(
        autofixed(src),
        "array Scores(3) s1-s3;\ntotal = Scores(1) + Scores(2);\n"
    );
}

#[test]
fn proc_step_bodies_are_left_alone() {
    // Option names, PROC-specific keywords and their arguments aren't
    // data-step variables, so a PROC's spelling never conflicts with one.
    let src = "proc sort data = Work.Src OUT = src;\n  by ID;\nrun;\n\
               proc means data = src NOPRINT;\n  var Score;\nrun;\n\
               data x;\n  set src;\n  y = id + score;\n  noprint = 1;\nrun;\n";
    assert!(flagged(src).is_empty(), "{:?}", flagged(src));
}

#[test]
fn a_run_less_proc_format_ends_at_the_first_data_step_statement() {
    // Fragments often open with `proc format; value …;` and run straight
    // into the data-step body. The format name `flagCap` is not a variable;
    // the body's `ageM` / `agem` are.
    let src = "proc format;\n  value flagCap 0 = 'No' 1 = 'Yes';\n\
               ageM = ageM;\nif agem > 3 then FLAGCAP = 1;\nrun;\n";
    assert_eq!(flagged(src), vec!["agem"]);
}

#[test]
fn statement_keywords_and_labels_are_not_variables() {
    // `retain`, `goto`, `link`, `return`, `options`, `title` lex as plain
    // identifiers; so do labels and infile options.
    let src = "RETAIN a;\nretain b;\n\
               Skip: a = 1;\nGOTO skip;\nlink SKIP;\nRETURN;\nreturn;\n\
               infile 'one' DLM = ',';\ninfile 'two' dlm = ',';\n\
               title 'x';\nTITLE 'y';\n";
    assert!(flagged(src).is_empty(), "{:?}", flagged(src));
}

#[test]
fn variables_are_still_caught_in_every_variable_position() {
    let src = "retain Total 0;\n\
               total = total + x;\n\
               if TOTAL > 3 then put total=;\n\
               else if x then Total + 1;\n\
               keep total;\n";
    // `total` is the majority spelling (4 uses vs 2 vs 1).
    assert_eq!(flagged(src), vec!["TOTAL", "Total", "Total"]);
}

#[test]
fn ties_resolve_to_the_first_spelling_in_the_file() {
    let src = "myVar = 1;\nMyVar = 2;\n";
    assert_eq!(flagged(src), vec!["MyVar"]);
    assert_eq!(autofixed(src), "myVar = 1;\nmyVar = 2;\n");
}
