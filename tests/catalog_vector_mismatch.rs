//! Focused suite for `catalog_vector_mismatch`: catalog rows whose values
//! column (`1-3,8`) and vector column (`1,2,3,4,5,6,7,8`) disagree. The
//! findings are about the catalog, so they come from `lint_inputs`, not
//! from linting a source.

use sas_linter::{Config, Linter};

const RULE: &str = "catalog_vector_mismatch";
const DIR: &str = "tests/fixtures/lints/catalog_vector_mismatch";

fn linter(yaml: &str) -> Linter {
    let config: Config = serde_yaml::from_str(yaml).expect("valid config");
    Linter::from_config(&config).expect("config builds")
}

fn report(yaml: &str) -> Vec<String> {
    linter(yaml)
        .lint_inputs()
        .iter()
        .map(|f| {
            assert_eq!(f.rule, RULE);
            format!("{}:{}: {}", f.path, f.line, f.message)
        })
        .collect()
}

fn shared_catalog(extra: &str) -> String {
    format!(
        "rules:\n  variable_value_out_of_known_range:\n    \
         csv_paths: [{DIR}/catalog.csv]\n    delimiter: \";\"\n{extra}"
    )
}

#[test]
fn reads_the_catalog_configured_for_variable_value_out_of_known_range() {
    assert_eq!(
        report(&shared_catalog("")),
        [
            format!(
                "{DIR}/catalog.csv:4: V3: Acceptable Values `1-3,8` and Vector \
                 `1,2,3,4,5,6,7,8` disagree: Vector adds 4-7"
            ),
            format!(
                "{DIR}/catalog.csv:6: V5: Vector `0,1` is a value list but \
                 Acceptable Values `with a stray delimiter` is not (shifted column?)"
            ),
            format!("{DIR}/catalog.csv:7: V6: Vector is empty but Acceptable Values is `0-4`"),
        ]
    );
}

#[test]
fn own_csv_paths_override_the_shared_catalog() {
    // `delimiter` is still inherited; only `csv_paths` is the rule's own.
    let yaml = shared_catalog(&format!("  {RULE}:\n    csv_paths: [{DIR}/agreeing.csv]\n"));
    assert_eq!(report(&yaml), Vec::<String>::new());
}

#[test]
fn a_catalog_without_the_vector_column_is_skipped() {
    let yaml = shared_catalog(&format!("  {RULE}:\n    vector_column: Missing\n"));
    assert_eq!(report(&yaml), Vec::<String>::new());

    let yaml = "rules:\n  variable_value_out_of_known_range:\n    \
                csv_paths: [tests/fixtures/lints/variable_value_out_of_known_range/variables.csv]\n    \
                delimiter: \";\"\n";
    assert_eq!(report(yaml), Vec::<String>::new());
}

#[test]
fn enabled_false_silences_it() {
    let yaml = shared_catalog(&format!("  {RULE}:\n    enabled: false\n"));
    assert_eq!(report(&yaml), Vec::<String>::new());
}

#[test]
fn silent_without_a_catalog() {
    let linter = Linter::from_ids(&[RULE.to_string()]).unwrap();
    assert!(linter.lint_inputs().is_empty());
    assert!(linter
        .lint("data a; x = 1; run;\n", "example.sas")
        .is_empty());
}

#[test]
fn sources_never_carry_catalog_findings() {
    let linter = linter(&shared_catalog(""));
    let findings = linter.lint("data a; x = 1; run;\n", "example.sas");
    assert!(findings.iter().all(|f| f.rule != RULE), "{findings:?}");
}
