//! Focused suite for the `unreachable_inner_branch_value` rule.
//!
//! An outer `if VAR in (S) then do;` guard restricts VAR inside the block.
//! Any inner test of VAR against a value outside S can never be true for
//! that value. The fixtures cover the simple nested-`if` shape; this suite
//! pins down the compound conditions the guarded blocks actually use.

use sas_linter::Linter;

const RULE: &str = "unreachable_inner_branch_value";

fn findings(src: &str) -> Vec<sas_linter::Finding> {
    let linter = Linter::from_ids(&[RULE.to_string()]).expect("rule id resolves");
    linter.lint(src, "example.sas")
}

fn positions(src: &str) -> Vec<(u32, u32)> {
    findings(src).iter().map(|f| (f.line, f.column)).collect()
}

#[test]
fn reports_a_dead_value_in_a_nested_if() {
    let src = "if e in (0,1,2,3,4,8) then do;\n\
               if e in (5,6) then x = 1;\n\
               end;\n";
    assert_eq!(positions(src), vec![(2, 10), (2, 12)]);
}

#[test]
fn reports_a_dead_value_in_an_else_if_arm() {
    let src = "if e in (0,1,2,3,4,8) then do;\n\
               if e in (0) then x = 1;\n\
               else if e in (9) then x = 2;\n\
               end;\n";
    assert_eq!(positions(src), vec![(3, 15)]);
}

// ── The gap from #16 ─────────────────────────────────────────────────────

#[test]
fn reports_a_dead_value_in_a_later_conjunct() {
    // The issue's shape: the guarded variable is not the first test in the
    // arm's `and` chain.
    let src = "if e in (0,1,2,3,4,8) then do;\n\
               if a in (0) and e in (3,4,8) then score = 5;\n\
               else if a in (0) and e in (3,4,5,8) then score = 6;\n\
               end;\n";
    let f = findings(src);
    assert_eq!(positions(src), vec![(3, 32)]);
    assert!(f[0].message.contains("value 5 for e"), "{}", f[0].message);
    assert!(f[0].message.contains("line 1"), "{}", f[0].message);
}

#[test]
fn checks_every_conjunct_of_a_condition() {
    let src = "if a in (0,1) then do;\n\
               if b = 1 and a in (2) and c = 3 and a = 4 then x = 1;\n\
               end;\n";
    assert_eq!(positions(src), vec![(2, 20), (2, 41)]);
}

#[test]
fn a_compound_guard_restricts_each_of_its_variables() {
    let src = "if a in (0) and b in (1,2,3,4) and c in (0) then do;\n\
               if a in (1) then x = 1;\n\
               else if b in (5) then x = 2;\n\
               else if c = 0 and b = 2 then x = 3;\n\
               end;\n";
    assert_eq!(positions(src), vec![(2, 10), (3, 15)]);
}

#[test]
fn an_inner_guard_is_checked_against_the_outer_one() {
    let src = "if e in (0,1,2) then do;\n\
               if e in (5) then do;\n\
               x = 1;\n\
               end;\n\
               end;\n";
    assert_eq!(positions(src), vec![(2, 10)]);
}

#[test]
fn an_inner_compound_guard_restricts_its_block_too() {
    let src = "if a in (0,1) then do;\n\
               if a in (0) and b in (1,2) then do;\n\
               if b in (3) then x = 1;\n\
               if a in (1) then x = 2;\n\
               end;\n\
               if a in (1) then x = 3;\n\
               end;\n";
    // b=3 dead under the inner guard; a=1 dead under the inner guard's
    // `a in (0)`; a=1 after the inner block is live again.
    assert_eq!(positions(src), vec![(3, 10), (4, 10)]);
    let f = findings(src);
    assert!(f[1].message.contains("line 2"), "{}", f[1].message);
}

// ── Message accuracy ─────────────────────────────────────────────────────

#[test]
fn distinguishes_a_dead_value_from_a_dead_branch() {
    let src = "if e in (0,1,2,3,4,8) then do;\n\
               if e in (3,4,5,8) then x = 1;\n\
               if e in (5,6) then x = 2;\n\
               if e = 9 then x = 3;\n\
               end;\n";
    let f = findings(src);
    assert_eq!(f.len(), 4, "{:?}", f);
    assert!(
        f[0].message
            .ends_with("this branch can never fire for that value."),
        "{}",
        f[0].message
    );
    assert!(
        f[1].message.ends_with("this branch is unreachable."),
        "{}",
        f[1].message
    );
    assert!(
        f[2].message.ends_with("this branch is unreachable."),
        "{}",
        f[2].message
    );
    assert!(
        f[3].message.ends_with("this branch is unreachable."),
        "{}",
        f[3].message
    );
}

#[test]
fn quotes_an_equality_guard_as_written() {
    let src = "data out;\n\
               set in;\n\
               if x = 4 then do;\n\
               if y in (0,1) then z=1;\n\
               else if x = 5 then z=2;\n\
               end;\n\
               run;\n";
    let f = findings(src);
    assert_eq!(f.len(), 1, "{:?}", f);
    assert_eq!(
        f[0].message,
        "value 5 for x is excluded by the enclosing `if x = 4` guard at line 3; \
         this branch is unreachable."
    );
}

#[test]
fn quotes_the_matching_conjunct_of_a_compound_guard() {
    let src = "if a in (0) and b in (1, 2) then do;\n\
               if b = 5 then x = 1;\n\
               end;\n";
    let f = findings(src);
    assert_eq!(f.len(), 1, "{:?}", f);
    assert!(
        f[0].message
            .contains("enclosing `if b in (1, 2)` guard at line 1"),
        "{}",
        f[0].message
    );
}

// ── What must stay silent ────────────────────────────────────────────────

#[test]
fn silent_when_every_value_is_allowed() {
    let src = "if e in (0,1,2,3,4,8) then do;\n\
               if a in (0) and e in (3,4,8) then score = 5;\n\
               else if a in (0) and e in (0,1,2) and age ge 80 then score = 4;\n\
               else if e = 8 then score = 3;\n\
               end;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn a_guard_with_a_disjunction_restricts_nothing() {
    // `a in (1) or e in (2)` does not constrain e on its own.
    let src = "if a in (1) or e in (2) then do;\n\
               if e in (5) then x = 1;\n\
               if a in (7) then x = 2;\n\
               end;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn an_inner_disjunction_is_not_reported() {
    // `e in (9) or a = 1` can still be true via `a = 1`, and whether the
    // dead disjunct matters is not this rule's call.
    let src = "if e in (0,1,2) then do;\n\
               if e in (9) or a = 1 then x = 1;\n\
               end;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn a_negated_test_is_not_reported() {
    let src = "if e in (0,1,2) then do;\n\
               if e not in (9) then x = 1;\n\
               if not e in (9) then x = 2;\n\
               if e ne 9 then x = 3;\n\
               end;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn the_guard_is_released_at_its_end() {
    let src = "if e in (0,1,2) then do;\n\
               x = 1;\n\
               end;\n\
               if e in (5) then x = 2;\n";
    assert!(findings(src).is_empty());
}

#[test]
fn a_guard_without_a_do_block_restricts_nothing() {
    let src = "if e in (0,1,2) then x = 1;\n\
               if e in (5) then x = 2;\n";
    assert!(findings(src).is_empty());
}

// ── Literal values ───────────────────────────────────────────────────────

#[test]
fn quote_style_does_not_change_a_string_value() {
    let src = "if c in ('A','B') then do;\n\
               if c = \"A\" then x = 1;\n\
               if c = \"C\" then x = 2;\n\
               end;\n";
    assert_eq!(positions(src), vec![(3, 8)]);
}

#[test]
fn trailing_blanks_do_not_change_a_string_value() {
    // SAS pads the shorter operand with blanks before comparing.
    let src = "if c in ('A ', 'B') then do;\n\
               if c = 'A' then x = 1;\n\
               if c in ('B  ', 'a') then x = 2;\n\
               end;\n";
    assert_eq!(positions(src), vec![(3, 17)]);
}

#[test]
fn doubled_quotes_are_one_quote() {
    let src = "if c in ('it''s') then do;\n\
               if c = \"it's\" then x = 1;\n\
               end;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn negative_and_exponent_literals_are_values() {
    let src = "if e in (-1, 1e2) then do;\n\
               if e in (-1, 100) then x = 1;\n\
               if e = -2 then x = 2;\n\
               end;\n";
    let f = findings(src);
    assert_eq!(positions(src), vec![(3, 8)]);
    assert!(f[0].message.contains("value -2 for e"), "{}", f[0].message);
}

#[test]
fn rule_is_report_only() {
    let meta = sas_linter::rules::all_metas();
    let m = meta.iter().find(|m| m.id == RULE).expect("rule registered");
    assert!(!m.supports_autofix);
}
