//! Focused suite for the `unbalanced_do_block` rule.
//!
//! The sources this linter targets are data-step *fragments*: the file holds
//! the body of a data step and the caller supplies `data …; set …;` and
//! `run;`. An unclosed `do` is therefore invisible until SAS compiles it,
//! where it fails with `ERROR 117-185: There were N unclosed DO blocks.` and
//! no line number. This rule names the specific `do` that never closed.

use sas_linter::Linter;

const RULE: &str = "unbalanced_do_block";

fn findings(src: &str) -> Vec<sas_linter::Finding> {
    let linter = Linter::from_ids(&[RULE.to_string()]).expect("rule id resolves");
    linter.lint(src, "test.sas")
}

fn positions(src: &str) -> Vec<(u32, u32)> {
    findings(src).iter().map(|f| (f.line, f.column)).collect()
}

#[test]
fn reports_each_unclosed_do_at_its_own_line() {
    // The issue's reduced repro: two `end`s dropped off the end of the body.
    let src = "if x = 0 then y = 0;\n\
               else do;\n   \
                  if z = 1 then y = 1;\n   \
                  else do;\n      \
                     y = 2;\n";
    let f = findings(src);
    assert_eq!(f.len(), 2, "expected one finding per unclosed do: {:?}", f);
    assert_eq!(positions(src), vec![(2, 6), (4, 9)]);
    assert!(f.iter().all(|f| f.message.contains("never closed")));
}

#[test]
fn silent_on_balanced_fragment() {
    let src = "if x = 0 then y = 0;\n\
               else do;\n   \
                  y = 1;\n\
               end;\n";
    assert!(findings(src).is_empty(), "balanced do/end must not fire");
}

#[test]
fn reports_stray_end() {
    let src = "do;\n  y = 1;\nend;\nend;\n";
    let f = findings(src);
    assert_eq!(f.len(), 1, "expected one stray-end finding: {:?}", f);
    assert_eq!((f[0].line, f[0].column), (4, 1));
    assert!(f[0].message.contains("closes nothing"), "{:?}", f[0]);
}

#[test]
fn every_do_variant_opens_a_block() {
    // Plain, iterative, while, until and array `do over` all close on one
    // `end` — each of these is missing it.
    for opener in [
        "do;",
        "do i = 1 to 10;",
        "do while (a < 3);",
        "do until (done);",
        "do over arr;",
    ] {
        let src = format!("{opener}\n  y = 1;\n");
        let f = findings(&src);
        assert_eq!(f.len(), 1, "`{opener}` should open a block: {:?}", f);
        assert_eq!(f[0].line, 1);
    }
}

#[test]
fn nested_do_variants_balance() {
    let src = "do i = 1 to 10;\n  \
                 do while (running < 100);\n    \
                   running = running + i;\n  \
                 end;\n\
               end;\n";
    assert!(findings(src).is_empty(), "nested variants must balance");
}

#[test]
fn data_step_select_consumes_an_end() {
    // `select` closes with `end` too. Counting `end`s without tracking what
    // opened them would mis-attribute this one.
    let balanced = "select (x);\n  when (1) y = 1;\n  otherwise y = 2;\nend;\n";
    assert!(
        findings(balanced).is_empty(),
        "balanced select must be quiet"
    );

    let unclosed = "select (x);\n  when (1) y = 1;\n  otherwise y = 2;\n";
    let f = findings(unclosed);
    assert_eq!(f.len(), 1, "unclosed select should fire once: {:?}", f);
    assert_eq!((f[0].line, f[0].column), (1, 1));
    assert!(f[0].message.contains("`select`"), "{:?}", f[0]);
}

#[test]
fn do_blocks_inside_select_branches_balance() {
    let src = "select (x);\n  \
                 when (1) do;\n    y = 1;\n  end;\n  \
                 otherwise do;\n    y = 9;\n  end;\n\
               end;\n";
    assert!(findings(src).is_empty(), "select branch do-blocks balance");
}

#[test]
fn proc_sql_select_puts_the_file_out_of_scope() {
    // A SQL `select` opens no block but `case … end` closes one, so `end` no
    // longer maps onto `do`. Stay silent rather than mis-report.
    let src = "proc sql;\n  select * from t;\nquit;\ndo;\n";
    assert!(
        findings(src).is_empty(),
        "files with a non-data-step select are out of scope"
    );
}

#[test]
fn macro_do_puts_the_file_out_of_scope() {
    // `%do` / `%end` are a separate nesting stack; don't mix the counters.
    let src = "%do i = 1 %to 3;\n  y = 1;\n%end;\ndo;\n";
    assert!(
        findings(src).is_empty(),
        "files using macro do-blocks are out of scope"
    );
}

#[test]
fn end_in_comments_and_strings_does_not_count() {
    let src = "do;\n  \
                 * end;\n  \
                 /* end; */\n  \
                 note = 'end;';\n\
               end;\n";
    assert!(
        findings(src).is_empty(),
        "`end` inside comments/strings must not close the block"
    );
}

#[test]
fn identifiers_containing_the_keyword_do_not_count() {
    let src = "weekend = 1;\nxEndDate = today();\nx = a_do;\n";
    assert!(
        findings(src).is_empty(),
        "word-boundary matching only — no substring hits"
    );
}

#[test]
fn end_as_a_data_step_option_or_variable_does_not_close() {
    // `set … end = eof;` names the end-of-file indicator; `if end then` reads
    // a variable called `end`. Neither is a block close.
    let src = "set source end = eof;\nif end then flush = 1;\n";
    assert!(
        findings(src).is_empty(),
        "`end` outside statement position must not be a close"
    );
}

#[test]
fn a_step_boundary_terminates_open_blocks() {
    // An `end;` in a later step must not be credited against a `do` left open
    // in an earlier one.
    let src = "data a;\n  do;\n    x = 1;\nrun;\n\ndata b;\n  y = 1;\nend;\nrun;\n";
    let f = findings(src);
    assert_eq!(
        f.len(),
        2,
        "expected an unclosed do and a stray end: {:?}",
        f
    );
    assert_eq!(f[0].line, 2, "unclosed do in the first step");
    assert_eq!(f[1].line, 8, "stray end in the second step");
}

#[test]
fn rule_is_report_only() {
    // Placing a missing `end` is a semantic decision — no autofix, even
    // opt-in. `--list-rules` must not advertise one.
    let meta = sas_linter::rules::all_metas();
    let m = meta.iter().find(|m| m.id == RULE).expect("rule registered");
    assert!(!m.supports_autofix, "rule must stay report-only");
}
