//! Formatter behavior — keyword casing, operator spacing, and
//! indentation. Sources are short generic snippets, written inline so
//! the test stays portable and contains no real variable codes.

use sas_linter::{Config, Formatter, Linter};

fn fmt_from_yaml(yaml: &str) -> Formatter {
    let cfg: Config = serde_yaml::from_str(yaml).expect("config parses");
    Formatter::from_config(&cfg).expect("formatter builds")
}

#[test]
fn preserve_is_noop() {
    let f = fmt_from_yaml(""); // empty config → defaults → preserve, no ops, no indent
    let src = "data foo;\n   if a=1 then b=2;\nrun;\n";
    assert_eq!(f.format(src), src);
}

#[test]
fn banner_alignment_is_on_by_default() {
    let f = fmt_from_yaml("");
    let src = "\
**  PROGRAM:      FOO.txt   **;\n\
**  BY:   ALICE                    **;\n";
    let out = f.format(src);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0].len(), lines[1].len(), "got:\n{out}");
    assert!(lines[0].contains("PROGRAM:  FOO.txt"), "got:\n{out}");
    assert!(lines[1].contains("BY:       ALICE"), "got:\n{out}");
}

#[test]
fn rectangular_banner_is_not_reflowed() {
    // A banner box whose lines all share one width was padded by hand;
    // the default banner pass must leave it byte-identical instead of
    // re-deriving its columns.
    let f = fmt_from_yaml("");
    let src = "\
*******************************************;\n\
**  PROGRAM:   EXAMPLE.txt               **;\n\
**  BY:              A. Person           **;\n\
**   Short description     xQ7b   0-5    **;\n\
*******************************************;\n";
    assert_eq!(f.format(src), src);
}

#[test]
fn assignment_alignment_is_on_by_default() {
    let f = fmt_from_yaml("");
    let src = "a    =    a;  **first;\n\nlonger  =  longer; **second;\n";
    let out = f.format(src);
    assert_eq!(
        out,
        "a      = a;       **first;\n\nlonger = longer;  **second;\n"
    );
}

#[test]
fn alignment_passes_can_be_disabled() {
    let f = fmt_from_yaml(
        r#"
format:
  align_comment_banners: false
  align_assignments: false
"#,
    );
    let src = "\
**  PROGRAM:      FOO.txt   **;\n\
**  BY:   ALICE                    **;\n\
a    =    a;  **first;\n\
longer  =  longer; **second;\n";
    assert_eq!(f.format(src), src);
}

#[test]
fn keywords_upper_uppercases_keywords_only() {
    let f = fmt_from_yaml(
        r#"
format:
  keywords: upper
"#,
    );
    let src = "data foo;\nrun;\n";
    let out = f.format(src);
    assert!(out.starts_with("DATA "));
    assert!(out.contains("RUN;"));
    // Identifier `foo` must stay lowercase.
    assert!(out.contains("foo"));
}

#[test]
fn operator_spacing_normalizes_around_assign() {
    let f = fmt_from_yaml(
        r#"
format:
  operator_spacing: true
"#,
    );
    let src = "data foo;\n   b=1+2;\nrun;\n";
    let out = f.format(src);
    assert!(out.contains("b = 1 + 2"), "got: {:?}", out);
}

#[test]
fn operator_spacing_preserves_unary_minus() {
    let f = fmt_from_yaml(
        r#"
format:
  operator_spacing: true
"#,
    );
    let src = "data foo;\n   b=-1;\nrun;\n";
    let out = f.format(src);
    // `b = -1`: space around `=` but no space between unary `-` and `1`.
    assert!(out.contains("b = -1"), "got: {:?}", out);
}

#[test]
fn indent_width_reflows_data_step_body() {
    let f = fmt_from_yaml(
        r#"
format:
  indent_width: 2
"#,
    );
    let src = "data foo;\nb=1;\nif a then do;\nc=2;\nend;\nrun;\n";
    let out = f.format(src);
    let expected = "data foo;\n  b=1;\n  if a then do;\n    c=2;\n  end;\nrun;\n";
    assert_eq!(out, expected, "got: {:?}", out);
}

#[test]
fn preserves_user_line_breaks_when_spacing() {
    let f = fmt_from_yaml(
        r#"
format:
  operator_spacing: true
"#,
    );
    // A line break between `=` and the RHS should be kept — operator
    // spacing only rewrites gaps that don't contain a newline.
    let src = "data foo;\nb =\n  1;\nrun;\n";
    let out = f.format(src);
    assert!(out.contains("=\n  1"), "got: {:?}", out);
}

fn format_with(yaml: &str, src: &str) -> String {
    let cfg: Config = serde_yaml::from_str(yaml).expect("config parses");
    let linter = Linter::from_config(&cfg).expect("linter builds");
    let formatter = Formatter::from_config(&cfg).expect("formatter builds");
    linter.format_source(src, &formatter)
}

const TAB_AUTOFIX: &str = "rules:\n  tab_expansion:\n    autofix: true\n";

#[test]
fn tab_expansion_runs_before_format_so_one_pass_settles() {
    // Alignment measured against a raw tab would be thrown off once the
    // tab is expanded, needing a second save to settle.
    let src = "data a;\n  x\t= 1;\n  longname = 2;\n  ab\t\t= 3;\nrun;\n";
    let once = format_with(TAB_AUTOFIX, src);
    assert!(!once.contains('\t'), "got:\n{once}");
    assert_eq!(format_with(TAB_AUTOFIX, &once), once);
    assert!(once.contains("  x        = 1;\n"), "got:\n{once}");
}

#[test]
fn hook_after_format_overrides_the_default() {
    let yaml = "rules:\n  tab_expansion:\n    autofix: true\n    hook: after_format\n";
    let src = "data a;\n\tx\t= 1;\n  longname = 2;\n  ab\t\t= 3;\nrun;\n";
    assert_ne!(format_with(yaml, src), format_with(TAB_AUTOFIX, src));
}

#[test]
fn unknown_hook_is_rejected() {
    let cfg: Config =
        serde_yaml::from_str("rules:\n  tab_expansion:\n    hook: sometime\n").unwrap();
    let err = Linter::from_config(&cfg).err().expect("bad hook errors");
    assert!(err.to_string().contains("tab_expansion.hook"), "got: {err}");
}
