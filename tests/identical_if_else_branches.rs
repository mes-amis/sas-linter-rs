//! Focused suite for the `identical_if_else_branches` rule: which
//! differences between the two bodies count.

use sas_linter::Linter;

const RULE: &str = "identical_if_else_branches";

fn lines(src: &str) -> Vec<u32> {
    let linter = Linter::from_ids(&[RULE.to_string()]).expect("rule id resolves");
    linter
        .lint(src, "example.sas")
        .into_iter()
        .map(|f| f.line)
        .collect()
}

#[test]
fn keyword_and_name_case_does_not_matter() {
    let src = "if a = 1 then LBL = 'Yes';\nelse lbl = 'Yes';\n";
    assert_eq!(lines(src), vec![2]);
}

#[test]
fn string_literal_case_matters() {
    let src = "if a = 1 then lbl = 'Yes';\nelse lbl = 'YES';\n";
    assert_eq!(lines(src), Vec::<u32>::new());
}
