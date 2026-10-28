//! Autofixes run in a fixed precedence (`rules::AUTOFIX_ORDER`), whatever
//! order a config lists the rules in, so one save settles: fixes that decide
//! what is comment and what is code run before fixes that rewrite the code.

use sas_linter::{Config, Linter};

fn linter(yaml: &str) -> Linter {
    let config: Config = serde_yaml::from_str(yaml).unwrap();
    Linter::from_config(&config).unwrap()
}

fn fix(linter: &Linter, src: &str) -> String {
    linter.lint_with_fixes(src, "t.sas").1
}

#[test]
fn every_autofix_rule_has_a_place_in_the_order() {
    let order = sas_linter::rules::AUTOFIX_ORDER;
    let metas = sas_linter::rules::all_metas();
    for m in metas.iter().filter(|m| m.supports_autofix) {
        assert_eq!(
            order.iter().filter(|id| **id == m.id).count(),
            1,
            "{} must appear once in AUTOFIX_ORDER",
            m.id
        );
    }
    for id in order {
        assert!(
            metas.iter().any(|m| m.id == *id && m.supports_autofix),
            "{} in AUTOFIX_ORDER is not an autofix rule",
            id
        );
    }
}

/// `x = 2;` is swallowed by the comment on line 2 until `unterminated_comment`
/// restores it. Counting casings before that picks `X`; after it, `x`.
const SWALLOWED: &str = "data a;\nX = 1; * note:\nx = 2;\nx = 3;\nrun;\n";
const SETTLED: &str = "data a;\nx = 1; * note:;\nx = 2;\nx = 3;\nrun;\n";

#[test]
fn comment_fixes_run_before_name_fixes_whatever_the_config_order() {
    // Config maps iterate alphabetically: inconsistent_variable_case < unterminated_comment.
    let l = linter(
        "rules:\n  inconsistent_variable_case:\n    autofix: true\n  unterminated_comment:\n    autofix: true\n",
    );
    assert_eq!(fix(&l, SWALLOWED), SETTLED);
}

#[test]
fn one_pass_settles() {
    let l = linter(
        "rules:\n  inconsistent_variable_case:\n    autofix: true\n  unterminated_comment:\n    autofix: true\n",
    );
    let once = fix(&l, SWALLOWED);
    assert_eq!(fix(&l, &once), once);
}
