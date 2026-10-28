//! Focused suite for the `encoding_issues` rule.
//!
//! Smart punctuation is folded to ASCII, but a fold that yields a quote
//! character must not change where SAS ends the surrounding token: inside
//! a string it is doubled (SAS's escape for a same-quoted quote), and in a
//! `*`/`%*` comment statement — where SAS also pairs quotes — it is doubled
//! when a single quote would be left unmatched.

use sas_linter::{Config, Linter};

fn linter(extra: &str) -> Linter {
    let yaml =
        format!("rules:\n  encoding_issues:\n    use_defaults: true\n    autofix: true\n{extra}");
    let config: Config = serde_yaml::from_str(&yaml).unwrap();
    let mut linter = Linter::from_config(&config).unwrap();
    linter.rules.retain(|r| r.id() == "encoding_issues");
    linter
}

fn fix(src: &str) -> String {
    linter("").lint_with_fixes(src, "test.sas").1
}

fn findings(src: &str) -> Vec<(u32, u32, String)> {
    linter("")
        .lint(src, "test.sas")
        .into_iter()
        .map(|f| (f.line, f.column, f.message))
        .collect()
}

#[test]
fn doubles_a_folded_quote_matching_the_string_delimiter() {
    assert_eq!(fix("x = 'It\u{2019}s';\n"), "x = 'It''s';\n");
    assert_eq!(
        fix("x = \"say \u{201C}hi\u{201D}\";\n"),
        "x = \"say \"\"hi\"\"\";\n"
    );
    // A macro-resolving string is double-quoted too.
    assert_eq!(
        fix("x = \"&mv. \u{201C}a\u{201D}\";\n"),
        "x = \"&mv. \"\"a\"\"\";\n"
    );
}

#[test]
fn leaves_a_folded_quote_single_when_it_differs_from_the_delimiter() {
    assert_eq!(
        fix("x = 'say \u{201C}hi\u{201D}';\ny = \"it\u{2019}s\";\n"),
        "x = 'say \"hi\"';\ny = \"it's\";\n"
    );
}

#[test]
fn folds_smart_quotes_used_as_delimiters_in_code() {
    assert_eq!(
        fix("label x = \u{2018}Pain scale\u{2019};\n"),
        "label x = 'Pain scale';\n"
    );
}

#[test]
fn keeps_quotes_balanced_in_comment_statements() {
    // SAS pairs quotes inside `*…;` comments, so a lone apostrophe would
    // swallow the code after it.
    assert_eq!(
        fix("* the patient\u{2019}s score;\nx = 1;\n"),
        "* the patient''s score;\nx = 1;\n"
    );
    assert_eq!(
        fix("* use \u{2018}A\u{2019} here;\nx = 1;\n"),
        "* use 'A' here;\nx = 1;\n"
    );
    // Block comments don't pair quotes.
    assert_eq!(fix("/* it\u{2019}s */\nx = 1;\n"), "/* it's */\nx = 1;\n");
}

#[test]
fn finding_reports_the_text_actually_written() {
    let f = findings("x = 'It\u{2019}s';\n");
    assert_eq!(f.len(), 1);
    assert_eq!((f[0].0, f[0].1), (1, 8));
    assert!(f[0].2.contains("\"''\""), "got: {}", f[0].2);
}

#[test]
fn does_not_rewrite_accented_capitals() {
    // U+00D0 / U+00D2..U+00D5 are Mac Roman mojibake for dashes and curly
    // quotes, but they are also real letters (Ð Ò Ó Ô Õ).
    let src = "name = '\u{00D3}SCAR'; city = 'S\u{00C3}O PAULO'; p = 'P\u{00D5}E';\n";
    assert_eq!(fix(src), src);
    // Still surfaced for a human to judge.
    let f = findings(src);
    assert_eq!(f.len(), 2, "{f:?}");
    assert!(f.iter().all(|f| f.2.contains("no autofix")), "{f:?}");
}

#[test]
fn win_1252_bytes_arrive_decoded_and_are_folded() {
    // `read_source` transcodes a non-UTF-8 file before any rule runs, so
    // Win-1252 0x92 reaches the rule as U+2019.
    let src = sas_linter::encoding::decode_bytes(b"x = 'It\x92s';\n");
    assert_eq!(fix(&src), "x = 'It''s';\n");
}

#[test]
fn user_replacements_keep_strings_valid() {
    let l = linter("    replacements:\n      \"\u{00B4}\": \"'\"\n");
    let fixed = l.lint_with_fixes("x = 'It\u{00B4}s';\n", "test.sas").1;
    assert_eq!(fixed, "x = 'It''s';\n");
}
