//! Focused suite for the `tab_expansion` rule.
//!
//! A TAB between statements is layout and can be expanded freely; a TAB
//! inside a string literal or a datalines block is data (`dlm="<TAB>"`,
//! tab-delimited card images) and must survive byte-for-byte.

use sas_linter::{Config, Linter};

fn linter(autofix: bool) -> Linter {
    let yaml = format!("rules:\n  tab_expansion:\n    autofix: {autofix}\n");
    let config: Config = serde_yaml::from_str(&yaml).unwrap();
    let mut linter = Linter::from_config(&config).unwrap();
    linter.rules.retain(|r| r.id() == "tab_expansion");
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
fn expands_layout_tabs_to_tab_stops() {
    assert_eq!(
        fix("\tx = 1;\nab\ty = 2;\n"),
        "        x = 1;\nab      y = 2;\n"
    );
}

#[test]
fn leaves_tabs_inside_string_literals() {
    let src = "infile in dlm=\"\t\";\nx = 'a\tb';\n\ty = \"&mv.\tz\";\n";
    assert_eq!(
        fix(src),
        "infile in dlm=\"\t\";\nx = 'a\tb';\n        y = \"&mv.\tz\";\n"
    );
    assert_eq!(positions(src), vec![(3, 1)]);
}

#[test]
fn leaves_tabs_inside_multi_line_strings() {
    let src = "x = \"one\n\ttwo\";\n\ty = 1;\n";
    assert_eq!(fix(src), "x = \"one\n\ttwo\";\n        y = 1;\n");
    assert_eq!(positions(src), vec![(3, 1)]);
}

#[test]
fn leaves_tabs_inside_datalines() {
    let src = "data a;\n\tinput x $ y $;\n\tdatalines;\nab\tcd\n;\nrun;\n";
    assert_eq!(
        fix(src),
        "data a;\n        input x $ y $;\n        datalines;\nab\tcd\n;\nrun;\n"
    );
    assert_eq!(positions(src), vec![(2, 1), (3, 1)]);
}

#[test]
fn tab_stop_counts_string_text_before_it() {
    // The string's characters still occupy columns on the line.
    assert_eq!(fix("x='ab';\ty=1;\n"), "x='ab';\ty=1;\n".replace('\t', " "));
}
