//! Focused suite for the `star_comment_swallows_code` rule.
//!
//! A SAS `*`-style comment runs to the next `;`, not to end-of-line. A
//! trailing `*` comment that ends a code line with `:` (a common typo for
//! `;`) therefore continues onto the following line and silently swallows
//! the next statement up to its terminating `;`. This rule flags that.

use sas_linter::Linter;

const RULE: &str = "star_comment_swallows_code";

fn findings(src: &str) -> Vec<sas_linter::Finding> {
    let linter = Linter::from_ids(&[RULE.to_string()]).expect("rule id resolves");
    linter.lint(src, "test.sas")
}

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
fn silent_when_trailing_comment_terminated_on_same_line() {
    // Proper trailing comment: terminates with `;` on its own line.
    let src = "x = 1; * a proper note;\ny = 2;\n";
    assert!(
        findings(src).is_empty(),
        "properly terminated trailing comment must not fire"
    );
}

#[test]
fn silent_on_full_line_comment_that_wraps() {
    // A full-line `*` comment (no code before it) that legitimately wraps
    // is not the high-signal trailing-comment form; do not fire.
    let src = "x = 1;\n* a full-line note that wraps\n  onto another line and ends;\ny = 2;\n";
    assert!(
        findings(src).is_empty(),
        "full-line wrapping comment must not fire"
    );
}

#[test]
fn silent_on_c_style_block_comment() {
    // `/* ... */` blocks are a different token type and terminate
    // explicitly; they never swallow code.
    let src = "x = 1; /* a c-style block\n   spanning lines */\ny = 2;\n";
    assert!(
        findings(src).is_empty(),
        "c-style block comment must not fire"
    );
}

#[test]
fn does_not_double_report_double_star_box() {
    // `**`-style visual boxes are owned by the `unterminated_comment`
    // rule; this rule leaves them alone.
    let src = "x = 1;\n** SOME HEADER **\ny = 2;\n";
    assert!(
        findings(src).is_empty(),
        "double-star box belongs to unterminated_comment, not this rule"
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
    let f = findings(src);
    let lines: Vec<u32> = f.iter().map(|x| x.line).collect();
    assert_eq!(
        lines,
        vec![2, 4],
        "should fire on the two `:`-ending comments"
    );
}

#[test]
fn message_mentions_the_missing_semicolon() {
    let src = "x = 1; * note:\ny = 2;\n";
    let f = findings(src);
    assert_eq!(f.len(), 1);
    let m = f[0].message.to_lowercase();
    assert!(
        m.contains("semicolon") || m.contains("`;`") || m.contains(';'),
        "message should reference the missing semicolon: {}",
        f[0].message
    );
}
