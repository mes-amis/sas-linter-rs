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
fn proc_sql_select_does_not_silence_the_rest_of_the_file() {
    // A SQL `select` opens no block. It used to switch the whole file off;
    // the data-step `do` after the PROC must still be reported.
    let src = "proc sql;\n  select * from t;\nquit;\ndo;\n";
    assert_eq!(positions(src), vec![(4, 1)]);
}

#[test]
fn sql_case_expressions_consume_their_end() {
    // `case … end` is SQL's only `end`, and it needn't end the statement:
    // `end as z,` / `end)` / `end;` all close the `case`.
    let src = "proc sql;\n  \
                 update t set x = case when y = 1 then 2 else 3 end;\n  \
                 create table u as\n    \
                   select case when a then 1 else 0 end as z,\n           \
                          sum(case b when 1 then 1 end) as n\n    \
                   from t;\n\
               quit;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn unclosed_sql_case_is_reported() {
    let src = "proc sql;\n  select case when a then 1 as z from t;\nquit;\n";
    let f = findings(src);
    assert_eq!(positions(src), vec![(2, 10)], "{:?}", f);
    assert!(f[0].message.contains("`case`"), "{:?}", f[0]);
}

#[test]
fn proc_template_define_and_edit_blocks_balance() {
    let src = "proc template;\n  \
                 define style styles.mine;\n    \
                   parent = styles.default;\n    \
                   class body / color = white;\n  \
                 end;\n  \
                 define table base.t;\n    \
                   define column c;\n      header = 'C';\n    end;\n  \
                 end;\n  \
                 edit base.freq;\n  end;\n\
               run;\n\
               data a;\n  y = 1;\nend;\nrun;\n";
    // Only the data step's stray `end` is real.
    assert_eq!(positions(src), vec![(16, 1)], "{:?}", findings(src));
}

#[test]
fn a_new_step_statement_terminates_a_run_less_data_step() {
    // `data b;` (and `proc …;`) end the previous step just as `run;` does.
    let src = "data a;\n  do;\n    x = 1;\ndata b;\n  y = 1;\nend;\nproc print;\nrun;\n";
    assert_eq!(positions(src), vec![(2, 3), (6, 1)], "{:?}", findings(src));
}

#[test]
fn run_group_procs_keep_their_context_across_run() {
    // PROC DS2: `data` and `run;` occur inside the PROC; `method … end;` is a
    // block. Nothing here is unbalanced.
    let src = "proc ds2;\n\
               data out;\n  \
                 method run();\n    set x;\n    if a then do;\n      b = 1;\n    end;\n  end;\n\
               enddata;\n\
               run;\n\
               quit;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn programming_procs_are_checked_like_a_data_step() {
    let balanced = "proc fcmp outlib=work.f.p;\n  \
                      function f(x);\n    do i = 1 to 2;\n      x = x + i;\n    end;\n    \
                      return(x);\n  endsub;\n\
                    run;\n";
    assert!(findings(balanced).is_empty(), "{:?}", findings(balanced));

    let unclosed = "proc fcmp outlib=work.f.p;\n  \
                      function f(x);\n    do i = 1 to 2;\n      x = x + i;\n    \
                      return(x);\n  endsub;\n\
                    run;\n";
    assert_eq!(
        positions(unclosed),
        vec![(3, 5)],
        "{:?}",
        findings(unclosed)
    );
}

#[test]
fn procs_without_a_block_model_are_not_checked() {
    // Embedded-language PROCs carry foreign `do` / `end` keywords.
    let src = "proc lua;\nsubmit;\n  for i = 1, 3 do\n    print(i)\n  end\nendsubmit;\nrun;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn balanced_macro_blocks_leave_the_file_in_scope() {
    // `%do` / `%end` are their own stack. When each macro block is balanced
    // in data-step terms, whether it's generated (or how often) can't change
    // the data-step balance, so the rest of the file is still checked.
    let src = "%do i = 1 %to 3;\n  if x then do;\n    y = 1;\n  end;\n%end;\ndo;\n";
    assert_eq!(positions(src), vec![(6, 1)], "{:?}", findings(src));
}

#[test]
fn macro_blocks_that_split_a_do_block_put_the_file_out_of_scope() {
    // Each `%if` arm opens a `do` that a shared `end;` closes — fine at
    // runtime, unbalanced at the token level. Stay silent.
    let src = "%if &a %then %do;\n  if x then do;\n%end;\n%else %do;\n  if y then do;\n%end;\n  z = 1;\nend;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));

    // Likewise a macro whose body opens a block its caller closes, and a
    // block keyword generated by a single-statement `%then`.
    for src in [
        "%macro open;\n  do;\n%mend;\n",
        "%if &a %then do;\n  y = 1;\nend;\nend;\n",
    ] {
        assert!(findings(src).is_empty(), "{src}: {:?}", findings(src));
    }
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
