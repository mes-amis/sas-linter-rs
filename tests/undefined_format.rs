//! Focused suite for `undefined_format`: a format attached to a variable
//! that neither the file's PROC FORMAT nor SAS itself defines.

use sas_linter::Linter;

const RULE: &str = "undefined_format";

fn findings(src: &str) -> Vec<sas_linter::Finding> {
    let linter = Linter::from_ids(&[RULE.to_string()]).expect("rule id resolves");
    linter.lint(src, "example.sas")
}

const FORMATS: &str = "proc format;\n\
                       value sixscore 0 = 'Independent';\n\
                       value $yn 'Y' = 'Yes';\n\
                       invalue inscore 'A' = 1;\n\
                       picture pct low-high = '009%';\n\
                       run;\n";

fn with_formats(body: &str) -> String {
    format!("{}data foo;\n{}\nrun;\n", FORMATS, body)
}

#[test]
fn flags_an_attrib_format_that_is_not_defined() {
    let src = with_formats("attrib sSCORE format = sscore.;");
    let f = findings(&src);
    assert_eq!(f.len(), 1, "{:?}", f);
    assert_eq!((f[0].line, f[0].column), (8, 24));
    assert!(f[0].message.contains("`sscore.`"), "{}", f[0].message);
}

#[test]
fn flags_format_and_informat_statements() {
    let src = with_formats("format a nope. b $nah.;\ninformat c nada10.;");
    let refs: Vec<String> = findings(&src)
        .iter()
        .map(|f| f.message.split('`').nth(1).unwrap().to_string())
        .collect();
    assert_eq!(refs, vec!["nope.", "$nah.", "nada10."]);
}

#[test]
fn a_dollar_is_part_of_the_name() {
    // `yn` is a character format; numeric `yn.` and `$sixscore.` are not defined.
    let src = with_formats("format a yn. b $sixscore.;");
    assert_eq!(findings(&src).len(), 2, "{:?}", findings(&src));
}

#[test]
fn defined_formats_match_case_insensitively_with_any_width() {
    let src = with_formats(
        "format a SIXSCORE. b $YN3. c pct8.;\n\
         informat d inscore. ;\n\
         attrib e format=sixscore10. informat=InScore.;",
    );
    assert!(findings(&src).is_empty(), "{:?}", findings(&src));
}

#[test]
fn builtin_formats_are_allowed() {
    let src = with_formats(
        "format a best. b best12. c 8. d 8.2 e $10. f $char20. g date9. \
         h yymmddn8. i dollar12.2 j comma10. k mmddyy10. l datetime20. \
         m percent8.1 n z5. o e8601da. p $upcase10.;\n\
         informat q anydtdte10. r $varying20.;",
    );
    assert!(findings(&src).is_empty(), "{:?}", findings(&src));
}

#[test]
fn a_file_without_proc_format_is_skipped() {
    // Its custom formats come from a catalog this file can't see.
    let src = "data foo;\n format a sscore.;\nrun;\n";
    assert!(findings(src).is_empty(), "{:?}", findings(src));
}

#[test]
fn a_file_searching_other_format_catalogs_is_skipped() {
    let src = format!(
        "options fmtsearch=(lib.formats);\n{}",
        with_formats("format a other.;")
    );
    assert!(findings(&src).is_empty(), "{:?}", findings(&src));
}

#[test]
fn removing_a_format_is_silent() {
    let src = with_formats("format a;\nattrib b length = 8;");
    assert!(findings(&src).is_empty(), "{:?}", findings(&src));
}
