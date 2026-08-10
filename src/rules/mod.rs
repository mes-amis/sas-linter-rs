use anyhow::{anyhow, Result};

use crate::config::Config;
use crate::rule::Rule;

pub mod choose_one_template;
pub mod commented_out_guard;
pub mod encoding_issues;
pub mod format_for_unknown_variable;
pub mod identical_if_else_branches;
pub mod inconsistent_variable_case;
pub mod invalid_assignment_target;
pub mod invalid_conventional_variable;
pub mod invalid_numeric_literal;
pub mod line_endings;
pub mod malformed_if_condition;
pub mod malformed_label_statement;
pub mod missing_assignment_semicolon;
pub mod source_headers;
pub mod star_comment_swallows_code;
pub mod tab_expansion;
pub mod trailing_whitespace;
pub mod unbalanced_do_block;
pub mod unreachable_inner_branch_value;
pub mod unterminated_comment;
pub mod variable_value_out_of_known_range;

/// Metadata for each rule, plus factory functions for default-config and
/// from-config construction. Central source of truth for `--list-rules`
/// and the Linter's config builder.
pub struct RuleMeta {
    pub id: &'static str,
    pub description: &'static str,
    pub supports_autofix: bool,
    pub default_factory: fn() -> Box<dyn Rule>,
    pub config_factory: fn(&serde_yaml::Value) -> Result<Box<dyn Rule>>,
}

pub fn all_metas() -> Vec<RuleMeta> {
    vec![
        unreachable_inner_branch_value::meta(),
        identical_if_else_branches::meta(),
        commented_out_guard::meta(),
        choose_one_template::meta(),
        trailing_whitespace::meta(),
        tab_expansion::meta(),
        source_headers::meta(),
        line_endings::meta(),
        encoding_issues::meta(),
        malformed_if_condition::meta(),
        missing_assignment_semicolon::meta(),
        invalid_assignment_target::meta(),
        malformed_label_statement::meta(),
        variable_value_out_of_known_range::meta(),
        invalid_conventional_variable::meta(),
        invalid_numeric_literal::meta(),
        inconsistent_variable_case::meta(),
        format_for_unknown_variable::meta(),
        unterminated_comment::meta(),
        star_comment_swallows_code::meta(),
        unbalanced_do_block::meta(),
    ]
}

pub fn default_rules() -> Vec<Box<dyn Rule>> {
    all_metas().iter().map(|m| (m.default_factory)()).collect()
}

pub fn rules_by_ids(ids: &[String]) -> Result<Vec<Box<dyn Rule>>> {
    let metas = all_metas();
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let m = metas.iter().find(|m| m.id == id.as_str()).ok_or_else(|| {
            let known: Vec<&str> = metas.iter().map(|m| m.id).collect();
            anyhow!("Unknown lint rule: {:?}. Known: {}", id, known.join(", "))
        })?;
        out.push((m.default_factory)());
    }
    Ok(out)
}

pub fn rules_from_config(config: &Config) -> Result<Vec<Box<dyn Rule>>> {
    let metas = all_metas();
    let mut out: Vec<Box<dyn Rule>> = Vec::new();

    // Pass 1: explicit entries, in config order.
    for (id, opts) in &config.rules {
        if opts.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
            continue;
        }
        let m = metas.iter().find(|m| m.id == id.as_str()).ok_or_else(|| {
            let known: Vec<&str> = metas.iter().map(|m| m.id).collect();
            anyhow!(
                "Unknown lint rule in config: {:?}. Known: {}",
                id,
                known.join(", ")
            )
        })?;
        out.push((m.config_factory)(opts)?);
    }

    // Pass 2: rules not mentioned in config — default options. Matches
    // the Ruby gem: omitted rules stay enabled so adding a new rule
    // doesn't silently regress existing setups.
    for m in &metas {
        if !config.rules.contains_key(m.id) {
            out.push((m.default_factory)());
        }
    }

    Ok(out)
}
