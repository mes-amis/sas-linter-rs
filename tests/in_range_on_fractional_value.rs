//! Focused suite for `in_range_on_fractional_value`: a colon range in an
//! `in` list matches whole numbers only, so `bmi in (5:70)` is false for
//! `28.4`.

use sas_linter::Linter;

const RULE: &str = "in_range_on_fractional_value";

fn findings(src: &str) -> Vec<sas_linter::Finding> {
    let linter = Linter::from_ids(&[RULE.to_string()]).expect("rule id resolves");
    linter.lint(src, "example.sas")
}

fn positions(src: &str) -> Vec<(u32, u32)> {
    findings(src).iter().map(|f| (f.line, f.column)).collect()
}

fn autofix(src: &str) -> String {
    let meta = sas_linter::rules::all_metas();
    let m = meta.iter().find(|m| m.id == RULE).expect("rule registered");
    let opts: serde_yaml::Value = serde_yaml::from_str("autofix: true").unwrap();
    (m.config_factory)(&opts).unwrap().autofix(src)
}

const ISSUE: &str = "bmi = weight/(height*height);\n\
                     age = floor(days / 365.25);\n\
                     if a in (0,1,2,3) and\n\
                     age in (0:130) and bmi in (5:70) then do;\n\
                     \x20  if bmi <18 then score=7;\n\
                     end;\n";

#[test]
fn flags_a_range_on_a_quotient() {
    let f = findings(ISSUE);
    assert_eq!(positions(ISSUE), vec![(4, 28)]);
    assert_eq!(
        f[0].message,
        "`bmi in (5:70)` only matches whole numbers; bmi is assigned a quotient \
         at line 1. Use `5 le bmi le 70` to include fractions."
    );
}

#[test]
fn whole_number_functions_hide_a_division() {
    for rhs in [
        "floor(a / b)",
        "int(a / b)",
        "ceil(a * 0.5)",
        "round(a / b)",
        "round(a / b, 1)",
        "n(a, b) + 1",
    ] {
        let src = format!("x = {};\nif x in (1:5) then y = 1;\n", rhs);
        assert!(findings(&src).is_empty(), "{}", rhs);
    }
}

#[test]
fn fraction_producing_assignments() {
    for (rhs, why) in [
        ("a / b", "assigned a quotient"),
        ("h * 0.0254", "assigned an expression with 0.0254"),
        ("sqrt(a)", "assigned the result of `sqrt`"),
        ("mean(a, b)", "assigned the result of `mean`"),
        ("a ** -1", "assigned a fractional power"),
        ("a ** 0.5", "assigned a fractional power"),
        ("round(a / b, 0.1)", "assigned a quotient"),
    ] {
        let src = format!("x = {};\nif x in (1:5) then y = 1;\n", rhs);
        let f = findings(&src);
        assert_eq!(f.len(), 1, "{}", rhs);
        assert!(f[0].message.contains(why), "{}: {}", rhs, f[0].message);
    }
    for rhs in ["a * b", "a ** 2", "a + 1", "1e3"] {
        let src = format!("x = {};\nif x in (1:5) then y = 1;\n", rhs);
        assert!(findings(&src).is_empty(), "{}", rhs);
    }
}

#[test]
fn any_fractional_assignment_counts() {
    let src = "if c then x = 1;\nelse x = a / b;\nif x in (1:5) then y = 1;\n";
    assert_eq!(positions(src), vec![(3, 10)]);
}

#[test]
fn comparison_with_a_fraction_counts() {
    let src = "if x < 18.5 then y = 1;\nif x in (5:70) then y = 2;\n";
    let f = findings(src);
    assert_eq!(positions(src), vec![(2, 10)]);
    assert!(
        f[0].message.contains("compared with 18.5 at line 1"),
        "{}",
        f[0].message
    );
    for cond in ["18.5 le x lt 30", "x in (1.5, 2)", "x ne -0.5"] {
        let src = format!("if {} then y = 1;\nif x in (5:70) then y = 2;\n", cond);
        assert_eq!(findings(&src).len(), 1, "{}", cond);
    }
    // Arithmetic around the literal: not a comparison of `x` itself.
    for cond in ["x < 0.5 * z", "z * x < 0.5", "x - 0.5 < z", "w < z - 0.5"] {
        let src = format!("if {} then y = 1;\nif x in (5:70) then y = 2;\n", cond);
        assert!(findings(&src).is_empty(), "{}", cond);
    }
}

#[test]
fn unknown_and_whole_variables_are_quiet() {
    for src in [
        "if x in (1:5) then y = 1;\n",
        "x = a + b;\nif x in (1:5) then y = 1;\n",
        "x = a / b;\nif x in (1, 2, 3) then y = 1;\n",
        "do i = 1 to n / 2;\n if i in (1:3) then y = 1;\nend;\n",
        "if x = a / b then y = 1;\nif x in (1:5) then y = 2;\n",
    ] {
        assert!(findings(src).is_empty(), "{}", src);
    }
}

#[test]
fn names_are_case_insensitive() {
    let src = "BMI = w / h;\nif bmi in (5:70) then y = 1;\n";
    assert_eq!(findings(src).len(), 1);
}

#[test]
fn reports_each_range_and_negations() {
    let src = "x = a / b;\nif x in (0, 1:3, 5:9) then y = 1;\nif x not in (-5:5) then y = 2;\n";
    assert_eq!(positions(src), vec![(2, 13), (2, 18), (3, 14)]);
    assert!(findings(src)[2].message.starts_with("`x not in (-5:5)`"));
}

#[test]
fn autofix_rewrites_a_lone_range() {
    assert_eq!(
        autofix("x = a / b;\nif c and x in (5:70) then y = 1;\n"),
        "x = a / b;\nif c and 5 le x le 70 then y = 1;\n"
    );
    assert_eq!(
        autofix("x = a / b;\nflag = x in (-5:5);\n"),
        "x = a / b;\nflag = -5 le x le 5;\n"
    );
    // Parenthesised where an operator binds tighter than the comparison.
    assert_eq!(
        autofix("x = a / b;\nflag = x in (1:5) + 1;\n"),
        "x = a / b;\nflag = (1 le x le 5) + 1;\n"
    );
}

#[test]
fn autofix_leaves_mixed_and_negated_lists() {
    for src in [
        "x = a / b;\nif x in (0, 5:70) then y = 1;\n",
        "x = a / b;\nif x not in (5:70) then y = 1;\n",
        "x = a + b;\nif x in (5:70) then y = 1;\n",
    ] {
        assert_eq!(autofix(src), src);
    }
}

#[test]
fn autofix_output_is_quiet() {
    let fixed = autofix(ISSUE);
    assert!(fixed.contains("5 le bmi le 70"), "{}", fixed);
    assert!(findings(&fixed).is_empty(), "{}", fixed);
}

fn configured(yaml: &str) -> sas_linter::Linter {
    let cfg: sas_linter::Config =
        serde_yaml::from_str(&format!("rules:\n  {}:\n{}", RULE, yaml)).expect("config parses");
    sas_linter::Linter::from_config(&cfg).expect("linter builds")
}

fn configured_findings(yaml: &str, src: &str) -> Vec<sas_linter::Finding> {
    configured(yaml)
        .lint(src, "example.sas")
        .into_iter()
        .filter(|f| f.rule == RULE)
        .collect()
}

#[test]
fn half_open_ranges_count_only_when_enabled() {
    let src = "if 18 le x lt 30 then y = 1;\nif x in (5:70) then y = 2;\n";
    assert!(findings(src).is_empty());
    let f = configured_findings("    half_open_ranges: true\n", src);
    assert_eq!(f.len(), 1);
    assert!(
        f[0].message
            .contains("compared as a half-open range (`18 le x lt 30`) at line 1"),
        "{}",
        f[0].message
    );
    for cond in ["30 gt x ge 18", "18 < x <= 30"] {
        let src = format!("if {} then y = 1;\nif x in (5:70) then y = 2;\n", cond);
        let f = configured_findings("    half_open_ranges: true\n", &src);
        assert_eq!(f.len(), 1, "{}", cond);
    }
    // Both bounds inclusive, mixed directions, or arithmetic alongside.
    for cond in ["18 le x le 30", "18 lt x gt 30", "18 le x lt 30 + z"] {
        let src = format!("if {} then y = 1;\nif x in (5:70) then y = 2;\n", cond);
        let f = configured_findings("    half_open_ranges: true\n", &src);
        assert!(f.is_empty(), "{}", cond);
    }
}

#[test]
fn decimal_catalog_entries_count() {
    let csv = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/lints/in_range_on_fractional_value/values.csv");
    let yaml = format!("    csv_paths:\n      - {}\n", csv.display());
    let src = "if bmi in (5:70) then y = 1;\nif count in (1:5) then y = 2;\n";
    assert!(findings(src).is_empty());
    let f = configured_findings(&yaml, src);
    assert_eq!(f.len(), 1, "{:?}", f);
    assert_eq!(
        f[0].message,
        "`bmi in (5:70)` only matches whole numbers; bmi is documented with decimal \
         values 0 to 99.9 in the values catalog. Use `5 le bmi le 70` to include fractions."
    );
}
