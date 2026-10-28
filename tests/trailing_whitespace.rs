//! Focused suite for the `trailing_whitespace` rule.
//!
//! Blanks before a newline are noise in code, but inside a multi-line
//! string literal they are part of the value, and in datalines they can
//! be part of a fixed-width card image. Those bytes must survive.

use sas_linter::{Config, Linter};

fn linter(autofix: bool) -> Linter {
    let yaml = format!("rules:\n  trailing_whitespace:\n    autofix: {autofix}\n");
    let config: Config = serde_yaml::from_str(&yaml).unwrap();
    let mut linter = Linter::from_config(&config).unwrap();
    linter.rules.retain(|r| r.id() == "trailing_whitespace");
    linter
}

fn fix(src: &str) -> String {
    linter(true).lint_with_fixes(src, "test.sas").1
}

fn positions(src: &str) -> Vec<(u32, u32)> {
    linter(false)
        .lint(src, "test.sas")
        .iter()
        .map(|f| (f.line, f.column))
        .collect()
}

#[test]
fn strips_trailing_blanks_in_code() {
    assert_eq!(
        fix("x = 1;  \r\ny = 2;\t\nz = 3; "),
        "x = 1;\r\ny = 2;\nz = 3;"
    );
}

#[test]
fn keeps_blanks_inside_multi_line_strings() {
    let src = "x = \"line one   \n line two\";   \ny = 'a \t\r\nb';\n";
    assert_eq!(
        fix(src),
        "x = \"line one   \n line two\";\ny = 'a \t\r\nb';\n"
    );
    assert_eq!(positions(src), vec![(2, 12)]);
}

#[test]
fn keeps_blanks_inside_macro_resolving_strings() {
    let src = "x = \"&mv. one  \ntwo\";\n";
    assert_eq!(fix(src), src);
    assert!(positions(src).is_empty());
}

#[test]
fn keeps_blanks_inside_datalines() {
    let src = "data a;\n  input x $8.;\n  datalines;  \nab  \ncd \n;\nrun;   \n";
    assert_eq!(
        fix(src),
        "data a;\n  input x $8.;\n  datalines;\nab  \ncd \n;\nrun;\n"
    );
    assert_eq!(positions(src), vec![(3, 13), (7, 5)]);
}
