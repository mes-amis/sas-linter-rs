//! Focused suite for the `line_endings` rule.

use sas_linter::Linter;

fn positions(src: &str) -> Vec<(u32, u32)> {
    Linter::from_ids(&["line_endings".to_string()])
        .unwrap()
        .lint(src, "test.sas")
        .iter()
        .map(|f| (f.line, f.column))
        .collect()
}

#[test]
fn reports_lone_and_double_cr() {
    assert_eq!(positions("x=1;\ry=2;\r\r\nz=3;\r\n"), vec![(1, 5), (2, 5)]);
}

#[test]
fn columns_count_characters_not_bytes() {
    // `é` and `’` are multi-byte in UTF-8 but one column each.
    assert_eq!(positions("x='é';\ry='’';\r\r\n"), vec![(1, 7), (2, 7)]);
}
