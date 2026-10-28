//! Keeps the README's YAML config template and built-in rules table in step
//! with the rule registry, so a new rule can't ship undocumented.

fn readme() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")).unwrap()
}

/// The fenced ```yaml block under `## YAML config`.
fn config_template(readme: &str) -> &str {
    let section = &readme[readme.find("## YAML config").expect("config section")..];
    let start = section.find("```yaml").expect("yaml block") + "```yaml".len();
    &section[start..start + section[start..].find("```").expect("closed block")]
}

#[test]
fn config_template_configures_every_rule() {
    let readme = readme();
    let template = config_template(&readme);
    let config: serde_yaml::Value = serde_yaml::from_str(template).expect("template parses");
    let rules = config["rules"].as_mapping().expect("rules mapping");
    let missing: Vec<&str> = sas_linter::rules::all_metas()
        .iter()
        .map(|m| m.id)
        .filter(|id| !rules.contains_key(*id))
        .collect();
    assert!(missing.is_empty(), "config template lacks: {:?}", missing);
}

#[test]
fn config_template_loads() {
    let readme = readme();
    let config: sas_linter::Config =
        serde_yaml::from_str(config_template(&readme)).expect("template is a valid config");
    sas_linter::Linter::from_config(&config).expect("every entry builds");
}

#[test]
fn rules_table_documents_every_rule() {
    let readme = readme();
    let missing: Vec<&str> = sas_linter::rules::all_metas()
        .iter()
        .map(|m| m.id)
        .filter(|id| !readme.contains(&format!("\n| `{}` |", id)))
        .collect();
    assert!(missing.is_empty(), "rules table lacks: {:?}", missing);
}
