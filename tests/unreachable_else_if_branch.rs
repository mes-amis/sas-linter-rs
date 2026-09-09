//! Focused suite for the `unreachable_else_if_branch` rule.
//!
//! Within one `if` / `else if` / `else` chain, an arm whose condition is
//! implied by an earlier arm can never fire: the earlier arm always wins.
//! SAS compiles such a chain without comment, so the linter is the only
//! place it gets caught.

use sas_linter::Linter;

const RULE: &str = "unreachable_else_if_branch";

fn findings(src: &str) -> Vec<sas_linter::Finding> {
    let linter = Linter::from_ids(&[RULE.to_string()]).expect("rule id resolves");
    linter.lint(src, "example.sas")
}

fn positions(src: &str) -> Vec<(u32, u32)> {
    findings(src).iter().map(|f| (f.line, f.column)).collect()
}

/// The issue's motivating case, verbatim: the last arm repeats the first.
const MOTIVATING: &str = "\
else if a in (0) and b in (1,2,3,4) and c in (0) and d in (1) and e in (3,4,8)              then score=5;
else if a in (0) and b in (1,2,3,4) and c in (0) and d in (1) and e in (0,1,2) and age ge 80 then score=4;
else if a in (0) and b in (1,2,3,4) and c in (0) and d in (1) and e in (0,1,2) and age lt 80 then score=3;
else if a in (0) and b in (1,2,3,4) and c in (0) and d in (2) and e in (0,1,2)              then score=3;
else if a in (0) and b in (1,2,3,4) and c in (0) and d in (2) and e in (3,4,8)              then score=4;
else if a in (0) and b in (1,2,3,4) and c in (0) and d in (1) and e in (3,4,8)              then score=5;
";

// ── Level 1: exact duplicates ────────────────────────────────────────────

#[test]
fn reports_the_motivating_case_with_the_expected_message() {
    let f = findings(MOTIVATING);
    assert_eq!(f.len(), 1, "exactly one unreachable arm: {:?}", f);
    assert_eq!(
        f[0].to_string(),
        "example.sas:6:6: [unreachable_else_if_branch] condition is identical to the \
         branch at line 1, so this branch can never fire"
    );
}

#[test]
fn silent_when_the_duplicate_arm_is_removed() {
    let distinct: String = MOTIVATING
        .lines()
        .take(5)
        .map(|l| format!("{l}\n"))
        .collect();
    assert!(
        findings(&distinct).is_empty(),
        "five distinct arms must be clean"
    );
}

#[test]
fn conjunct_order_and_set_order_do_not_matter() {
    let src = "if a in (1,2) and b = 3 then x = 1;\n\
               else if b eq 3 and a in (2, 1) then x = 2;\n";
    assert_eq!(positions(src), vec![(2, 6)]);
    assert!(findings(src)[0]
        .message
        .contains("identical to the branch at line 1"));
}

#[test]
fn keyword_and_identifier_case_do_not_matter() {
    let src = "IF A IN (1) THEN X = 1;\n\
               else if a in ( 1 ) then x = 2;\n";
    assert_eq!(positions(src), vec![(2, 6)]);
}

#[test]
fn eq_and_single_element_in_are_the_same_test() {
    let src = "if a = 1 then x = 1;\n\
               else if a in (1) then x = 2;\n\
               else if a eq 1 then x = 3;\n";
    assert_eq!(positions(src), vec![(2, 6), (3, 6)]);
}

#[test]
fn one_finding_per_unreachable_arm_each_naming_the_arm_that_shadows_it() {
    let src = "if a = 1 then x = 1;\n\
               else if a = 2 then x = 2;\n\
               else if a = 1 then x = 3;\n\
               else if a = 2 then x = 4;\n";
    let f = findings(src);
    assert_eq!(positions(src), vec![(3, 6), (4, 6)]);
    assert!(f[0].message.contains("line 1"), "{}", f[0].message);
    assert!(f[1].message.contains("line 2"), "{}", f[1].message);
}

#[test]
fn arms_with_do_blocks_are_compared_too() {
    let src = "if a = 1 then do;\n  x = 1;\nend;\n\
               else if a = 2 then do;\n  x = 2;\nend;\n\
               else if a = 1 then do;\n  x = 3;\nend;\n";
    assert_eq!(positions(src), vec![(7, 6)]);
}

// ── Level 2: subsumption ─────────────────────────────────────────────────

#[test]
fn smaller_in_set_after_a_superset_is_unreachable() {
    let src = "if a in (0) and e in (3,4,8) then x = 5;\n\
               else if a in (0) and e in (3,4) then x = 6;\n";
    let f = findings(src);
    assert_eq!(positions(src), vec![(2, 6)]);
    assert_eq!(
        f[0].to_string(),
        "example.sas:2:6: [unreachable_else_if_branch] condition is fully covered by the \
         branch at line 1, so this branch can never fire"
    );
}

#[test]
fn larger_in_set_after_a_subset_is_reachable() {
    let src = "if a in (0) and e in (3,4) then x = 5;\n\
               else if a in (0) and e in (3,4,8) then x = 6;\n";
    assert!(findings(src).is_empty());
}

#[test]
fn narrower_range_after_a_wider_one_is_unreachable() {
    let src = "if age ge 80 then x = 1;\n\
               else if age ge 85 then x = 2;\n\
               else if age gt 90 then x = 3;\n";
    assert_eq!(positions(src), vec![(2, 6), (3, 6)]);

    let src = "if age lt 80 then x = 1;\n\
               else if age le 70 then x = 2;\n";
    assert_eq!(positions(src), vec![(2, 6)]);

    let src = "if age < 80 then x = 1;\n\
               else if age <= 70 then x = 2;\n";
    assert_eq!(positions(src), vec![(2, 6)]);
}

#[test]
fn ranges_that_split_the_line_are_reachable() {
    let src = "if age ge 80 then x = 1;\n\
               else if age lt 80 then x = 2;\n";
    assert!(findings(src).is_empty());

    // Wider after narrower is fine too.
    let src = "if age ge 85 then x = 1;\n\
               else if age ge 80 then x = 2;\n";
    assert!(findings(src).is_empty());

    // Strict vs inclusive bound at the same value: `ge 80` is not covered
    // by `gt 80`.
    let src = "if age gt 80 then x = 1;\n\
               else if age ge 80 then x = 2;\n";
    assert!(findings(src).is_empty());
}

#[test]
fn point_values_inside_an_earlier_range_are_unreachable() {
    let src = "if age ge 80 then x = 1;\n\
               else if age = 90 then x = 2;\n\
               else if age in (81, 82) then x = 3;\n\
               else if age in (79, 81) then x = 4;\n";
    assert_eq!(positions(src), vec![(2, 6), (3, 6)]);
}

#[test]
fn a_stricter_later_arm_is_unreachable_but_a_looser_one_is_not() {
    let src = "if a = 1 then x = 1;\n\
               else if a = 1 and b = 2 then x = 2;\n";
    assert_eq!(positions(src), vec![(2, 6)]);

    let src = "if a = 1 and b = 2 then x = 1;\n\
               else if a = 1 then x = 2;\n";
    assert!(findings(src).is_empty());
}

#[test]
fn different_variables_never_shadow_each_other() {
    let src = "if a in (1,2,3) then x = 1;\n\
               else if b in (1,2) then x = 2;\n";
    assert!(findings(src).is_empty());
}

#[test]
fn string_and_numeric_values_are_distinct() {
    let src = "if a in ('1', '2') then x = 1;\n\
               else if a in (1) then x = 2;\n\
               else if a = '2' then x = 3;\n";
    assert_eq!(positions(src), vec![(3, 6)]);
}

// ── Opaque arms ──────────────────────────────────────────────────────────

#[test]
fn arms_with_function_calls_are_never_reported() {
    // `lag` and `rand` return different values on each call, so a repeated
    // condition is not a repeated test.
    let src = "if lag(x) = 1 then y = 1;\n\
               else if lag(x) = 1 then y = 2;\n\
               else if a = 1 and rand('uniform') < 0.5 then y = 3;\n\
               else if a = 1 and rand('uniform') < 0.5 then y = 4;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn opaque_conjuncts_only_match_themselves_exactly() {
    // Arithmetic and `or` are outside the reasoner, but an exact repeat is
    // still an exact repeat.
    let src = "if a + b = 3 then x = 1;\n\
               else if a + b = 3 then x = 2;\n\
               else if c = 1 or d = 2 then x = 3;\n\
               else if c = 1 or d = 2 then x = 4;\n";
    assert_eq!(positions(src), vec![(2, 6), (4, 6)]);

    // `and` binds tighter than `or`, so conjunct reordering across an `or`
    // is a different condition.
    let src = "if a = 1 and b = 2 or c = 3 then x = 1;\n\
               else if c = 3 and b = 2 or a = 1 then x = 2;\n";
    assert!(findings(src).is_empty());

    // Nor does a subset reasoner apply through an `or`.
    let src = "if a in (1,2) or b = 1 then x = 1;\n\
               else if a in (1) or b = 1 then x = 2;\n";
    assert!(findings(src).is_empty());
}

#[test]
fn negations_are_opaque() {
    let src = "if not a = 1 then x = 1;\n\
               else if a ne 1 then x = 2;\n\
               else if a not in (1, 2) then x = 3;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

// ── Chain boundaries ─────────────────────────────────────────────────────

#[test]
fn a_new_if_starts_a_new_chain() {
    let src = "if a = 1 then x = 1;\n\
               if a = 1 then x = 2;\n\
               else if a = 2 then x = 3;\n";
    assert!(findings(src).is_empty());
}

#[test]
fn a_bare_else_ends_the_chain_and_is_never_reported() {
    let src = "if a = 1 then x = 1;\n\
               else x = 2;\n\
               if a = 1 then x = 3;\n\
               else do;\n  x = 4;\nend;\n";
    assert!(findings(src).is_empty());
}

#[test]
fn nested_chains_are_checked_on_their_own() {
    let src = "if a in (1,2) then do;\n\
                  if b = 1 then x = 1;\n\
                  else if b = 1 then x = 2;\n\
               end;\n\
               else if a in (1) then x = 3;\n";
    assert_eq!(positions(src), vec![(3, 6), (5, 6)]);
}

#[test]
fn an_inner_chain_does_not_see_the_outer_guard() {
    // `a in (1)` inside the `a in (1,2)` block is the other rule's
    // business (`unreachable_inner_branch_value`), not this one's.
    let src = "if a in (1,2) then do;\n\
                  if a in (1) then x = 1;\n\
                  else if a in (2) then x = 2;\n\
               end;\n";
    assert!(findings(src).is_empty());
}

#[test]
fn a_do_block_arm_does_not_leak_its_inner_chain() {
    // The inner chain closes with the `end;`; the outer `else if` belongs
    // to the outer chain and is compared against `a = 1`, not `b = 1`.
    let src = "if a = 1 then do;\n\
                  if b = 1 then x = 1;\n\
               end;\n\
               else if b = 1 then x = 2;\n\
               else if a = 1 then x = 3;\n";
    assert_eq!(positions(src), vec![(5, 6)]);
}

#[test]
fn select_blocks_inside_an_arm_do_not_break_the_chain() {
    let src = "if a = 1 then do;\n\
                  select (b);\n\
                     when (1) x = 1;\n\
                     otherwise do;\n\
                        x = 2;\n\
                     end;\n\
                  end;\n\
               end;\n\
               else if a = 1 then x = 3;\n";
    assert_eq!(positions(src), vec![(9, 6)]);
}

#[test]
fn iterative_do_inside_an_arm_does_not_break_the_chain() {
    let src = "if a = 1 then do;\n\
                  do i = 1 to 3;\n\
                     x = i;\n\
                  end;\n\
                  set b end = eof;\n\
               end;\n\
               else if a = 1 then x = 3;\n";
    assert_eq!(positions(src), vec![(7, 6)]);
}

#[test]
fn comments_and_string_literals_do_not_take_part() {
    let src = "if a = 1 then x = 'else if a = 1 then';\n\
               /* else if a = 1 then x = 2; */\n\
               * else if a = 1 then x = 3;\n\
               else if a = 2 then x = 4;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn malformed_chains_are_left_alone() {
    // No `then` — `malformed_if_condition` owns this.
    let src = "if a = 1 x = 1;\n\
               else if a = 1 then x = 2;\n";
    assert!(findings(src).is_empty());

    // Empty condition.
    let src = "if then x = 1;\n\
               else if then x = 2;\n";
    assert!(findings(src).is_empty());
}

#[test]
fn rule_is_report_only() {
    // Whether the duplicate arm is a leftover to delete or a branch meant
    // to test something else is the author's call. `--list-rules` must not
    // advertise an autofix.
    let meta = sas_linter::rules::all_metas();
    let m = meta.iter().find(|m| m.id == RULE).expect("rule registered");
    assert!(!m.supports_autofix, "rule must stay report-only");
}
