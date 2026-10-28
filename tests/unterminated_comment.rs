//! Focused suite for the `unterminated_comment` rule.
//!
//! A SAS `*` / `**` comment runs to the next `;`, not to end-of-line. One
//! whose first line has no `;` (often a `:` typo, or a `** … **` box line
//! missing its `;`) continues onto the following lines and silently
//! swallows any code there up to its `;`. The rule fires when a swallowed
//! line is a SAS statement — whatever the comment's style — and stays quiet
//! when the comment only wraps prose.

use sas_linter::{Config, Linter, Severity};

const RULE: &str = "unterminated_comment";

fn findings(src: &str) -> Vec<sas_linter::Finding> {
    let linter = Linter::from_ids(&[RULE.to_string()]).expect("rule id resolves");
    linter.lint(src, "test.sas")
}

fn lines(src: &str) -> Vec<u32> {
    findings(src).iter().map(|f| f.line).collect()
}

fn autofix(src: &str) -> String {
    let meta = sas_linter::rules::all_metas();
    let m = meta.iter().find(|m| m.id == RULE).expect("rule registered");
    let opts: serde_yaml::Value = serde_yaml::from_str("autofix: true").unwrap();
    (m.config_factory)(&opts).unwrap().autofix(src)
}

// ── Comments that swallow code ───────────────────────────────────────────

#[test]
fn fires_on_trailing_star_comment_missing_semicolon() {
    // The comment on line 1 ends in `:` and swallows `y = 2;` on line 2.
    let src = "x = 1; * note ending in colon:\ny = 2;\n";
    let f = findings(src);
    assert_eq!(f.len(), 1, "expected one finding, got: {:?}", f);
    assert_eq!(f[0].rule, RULE);
    assert_eq!(f[0].line, 1, "finding should point at the opening line");
}

#[test]
fn fires_on_a_double_star_box_line_missing_semicolon() {
    assert_eq!(lines("x = 1;\n** SOME HEADER **\ny = 2;\n"), vec![2]);
}

#[test]
fn fires_on_a_trailing_double_star_comment() {
    assert_eq!(lines("x = 1; ** note\ny = 2;\n"), vec![1]);
}

#[test]
fn fires_on_a_full_line_comment_that_swallows_code() {
    assert_eq!(lines("x = 1;\n** note\nz = 3;\n"), vec![2]);
    assert_eq!(lines("x = 1;\n* note\nz = 3;\n"), vec![2]);
}

#[test]
fn fires_when_code_follows_wrapped_prose() {
    assert_eq!(
        lines("* a note that wraps\n  onto a second line\ny = 2;\n"),
        vec![1]
    );
}

#[test]
fn flags_each_unterminated_comment_in_a_run_of_statements() {
    // Mirrors the motivating real-world defect (anonymized): several
    // deficit-style statements each carry a trailing comment, two of which
    // end in `:` and swallow the following statement.
    let src = "\
total = 0;
if a1 = 1 then total = total + 1; * first:
if a2 = 1 then total = total + 1; * second;
if a3 = 1 then total = total + 1; * third:
if a4 = 1 then total = total + 1; * fourth;
";
    assert_eq!(
        lines(src),
        vec![2, 4],
        "should fire on the two `:`-ending comments"
    );
}

#[test]
fn message_names_the_swallowed_line_and_the_missing_semicolon() {
    let f = findings("x = 1; * note:\ny = 2;\n");
    assert_eq!(f.len(), 1);
    assert!(f[0].message.contains("`;`"), "{}", f[0].message);
    assert!(f[0].message.contains("line 2"), "{}", f[0].message);
}

#[test]
fn swallowed_code_is_an_error() {
    for src in ["x = 1; * note:\ny = 2;\n", "x = 1;\n** BOX **\ny = 2;\n"] {
        assert_eq!(findings(src)[0].severity, Severity::Error, "{}", src);
    }
}

// ── Comments that only wrap prose ────────────────────────────────────────

#[test]
fn silent_when_trailing_comment_terminated_on_same_line() {
    assert!(findings("x = 1; * a proper note;\ny = 2;\n").is_empty());
}

#[test]
fn silent_on_full_line_comment_that_wraps() {
    let src = "x = 1;\n* a full-line note that wraps\n  onto another line and ends;\ny = 2;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn silent_on_a_header_box() {
    // The box's rows are one comment up to the closing `**;`.
    let src = "*****************************************;\n\
               ** PROGRAM:  scores                    **\n\
               ** PURPOSE:  compute the summary score **\n\
               *****************************************;\n\
               data foo;\nrun;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
    let boxed = "*****************************************\n\
                 ** PROGRAM:  scores                    **\n\
                 *****************************************;\n";
    assert!(findings(boxed).is_empty(), "{:?}", findings(boxed));
}

#[test]
fn silent_when_the_comment_runs_to_end_of_file() {
    // With no `;` left, nothing it swallowed was a complete SAS statement —
    // e.g. a trailing SPSS section a `**` banner comments out.
    let src = "run;\n**  START OF SPSS CODE  **.\n\ncount c = a b (0).\nif (a eq 0) s=1.\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn silent_on_c_style_block_comment() {
    let src = "x = 1; /* a c-style block\n   spanning lines */\ny = 2;\n";
    assert!(findings(src).is_empty());
}

// ── Autofix ──────────────────────────────────────────────────────────────

#[test]
fn autofix_terminates_the_comment_on_its_first_line() {
    assert_eq!(
        autofix("x = 1; * note:\ny = 2;\n"),
        "x = 1; * note:;\ny = 2;\n"
    );
    assert_eq!(
        autofix("data x;\r\n** COMMENT ** \t\r\nx = 1;\r\nrun;\r\n"),
        "data x;\r\n** COMMENT **;\r\nx = 1;\r\nrun;\r\n"
    );
}

#[test]
fn autofix_leaves_a_comment_whose_next_line_is_prose() {
    // Ending the comment on line 1 would turn line 2's prose into code.
    let src = "* a note that wraps\n  onto a second line\ny = 2;\n";
    assert_eq!(autofix(src), src);
}

#[test]
fn autofix_leaves_a_header_box_alone() {
    let src = "*****************************************\n\
               ** PROGRAM:  scores                    **\n\
               *****************************************;\n";
    assert_eq!(autofix(src), src);
}

// ── The retired `star_comment_swallows_code` id ──────────────────────────

#[test]
fn the_retired_id_runs_this_rule() {
    let linter = Linter::from_ids(&["star_comment_swallows_code".to_string()]).unwrap();
    let f = linter.lint("x = 1; * note:\ny = 2;\n", "t.sas");
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].rule, RULE);
}

#[test]
fn configuring_both_ids_runs_the_rule_once() {
    let yaml = "rules:\n  unterminated_comment:\n    enabled: true\n  star_comment_swallows_code:\n    enabled: true\n";
    let config: Config = serde_yaml::from_str(yaml).unwrap();
    let linter = Linter::from_config(&config).unwrap();
    let f = linter.lint("x = 1; * note:\ny = 2;\n", "t.sas");
    assert_eq!(f.iter().filter(|f| f.rule == RULE).count(), 1, "{:?}", f);
}

#[test]
fn the_current_id_wins_over_the_retired_one() {
    let yaml = "rules:\n  unterminated_comment:\n    enabled: true\n  star_comment_swallows_code:\n    enabled: false\n";
    let config: Config = serde_yaml::from_str(yaml).unwrap();
    let linter = Linter::from_config(&config).unwrap();
    assert_eq!(linter.lint("x = 1; * note:\ny = 2;\n", "t.sas").len(), 1);
}

#[test]
fn the_retired_id_is_not_listed() {
    assert!(!sas_linter::rules::all_metas()
        .iter()
        .any(|m| m.id == "star_comment_swallows_code"));
}
