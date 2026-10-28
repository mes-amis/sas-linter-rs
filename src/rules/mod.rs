use anyhow::{anyhow, Result};

use crate::config::{opt_str, Config};
use crate::finding::Finding;
use crate::rule::{CheckContext, Hook, Rule};

pub mod choose_one_template;
pub mod commented_out_guard;
pub mod condition;
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
pub mod orphan_format_range;
pub mod proc_statement_missing_semicolon;
pub mod source_headers;
pub mod statement_not_valid;
pub mod tab_expansion;
pub mod trailing_whitespace;
pub mod unbalanced_do_block;
pub mod undefined_format;
pub mod unescaped_quote_in_string;
pub mod unreachable_else_if_branch;
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
        unbalanced_do_block::meta(),
        unreachable_else_if_branch::meta(),
        proc_statement_missing_semicolon::meta(),
        undefined_format::meta(),
        orphan_format_range::meta(),
        statement_not_valid::meta(),
        unescaped_quote_in_string::meta(),
    ]
}

/// The order autofixes run in, whatever order the config lists rules in.
/// Each phase settles something the next one relies on, so one save
/// reaches a fixed point:
pub const AUTOFIX_ORDER: &[&str] = &[
    // 1. Bytes: encoding, line endings and blanks, so later byte and
    //    column math sees the final text.
    "encoding_issues",
    "line_endings",
    "tab_expansion",
    "trailing_whitespace",
    // 2. String and comment boundaries: what is string, comment and code.
    //    A runaway string can swallow `;` and comments, so strings go
    //    first. Headers are rewrapped next; terminating an assignment can
    //    leave a comment that swallows the next line, which
    //    `unterminated_comment` closes.
    "unescaped_quote_in_string",
    "source_headers",
    "missing_assignment_semicolon",
    "unterminated_comment",
    // 3. Statement boundaries inside the code.
    "proc_statement_missing_semicolon",
    "malformed_label_statement",
    "orphan_format_range",
    // 4. Names, once the code they appear in is final.
    "invalid_assignment_target",
    "inconsistent_variable_case",
];

/// When a rule reports a line, findings on that line from the rules listed
/// after it are dropped: the first rule names the actual defect, and the
/// others are tripping over its consequences. Only applies while the first
/// rule is enabled, so the line is never left unreported.
pub const REPORT_PRECEDENCE: &[(&str, &[&str])] = &[
    // Header text running as code reads as an invalid statement, a broken
    // `if` and stray identifiers.
    (
        "source_headers",
        &[
            "statement_not_valid",
            "malformed_if_condition",
            "inconsistent_variable_case",
        ],
    ),
    // The text after an early-closing quote reads as stray code.
    (
        "unescaped_quote_in_string",
        &[
            "malformed_label_statement",
            "statement_not_valid",
            "malformed_if_condition",
            "inconsistent_variable_case",
        ],
    ),
    // `Predicted risk = …` is an invalid statement too; this one names the fix.
    ("invalid_assignment_target", &["statement_not_valid"]),
];

/// Rule ids folded into another rule, mapped to the id that now covers
/// them. Configs and `--rules` lists naming a retired id keep working.
const RETIRED_IDS: &[(&str, &str)] = &[("star_comment_swallows_code", "unterminated_comment")];

fn current_id(id: &str) -> &str {
    RETIRED_IDS
        .iter()
        .find(|(old, _)| *old == id)
        .map_or(id, |(_, new)| new)
}

pub fn default_rules() -> Vec<Box<dyn Rule>> {
    all_metas().iter().map(|m| (m.default_factory)()).collect()
}

pub fn rules_by_ids(ids: &[String]) -> Result<Vec<Box<dyn Rule>>> {
    let metas = all_metas();
    let mut out = Vec::with_capacity(ids.len());
    let mut seen = std::collections::HashSet::new();
    for id in ids {
        let id = current_id(id);
        if !seen.insert(id) {
            continue;
        }
        let m = metas.iter().find(|m| m.id == id).ok_or_else(|| {
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

    // Pass 1: explicit entries, in config order. A retired id configures
    // the rule that replaced it, unless that rule has its own entry.
    for (id, opts) in &config.rules {
        let current = current_id(id);
        if current != id.as_str() && config.rules.contains_key(current) {
            continue;
        }
        if opts.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
            continue;
        }
        let m = metas.iter().find(|m| m.id == current).ok_or_else(|| {
            let known: Vec<&str> = metas.iter().map(|m| m.id).collect();
            anyhow!(
                "Unknown lint rule in config: {:?}. Known: {}",
                id,
                known.join(", ")
            )
        })?;
        let rule = (m.config_factory)(opts)?;
        out.push(match opt_str(opts, "hook") {
            Some(h) => Box::new(HookOverride {
                inner: rule,
                hook: Hook::parse(h).map_err(|e| anyhow!("{}.{}", id, e))?,
            }),
            None => rule,
        });
    }

    // Pass 2: rules not mentioned in config — default options. Matches
    // the Ruby gem: omitted rules stay enabled so adding a new rule
    // doesn't silently regress existing setups.
    let configured: std::collections::HashSet<&str> =
        config.rules.keys().map(|id| current_id(id)).collect();
    for m in &metas {
        if !configured.contains(m.id) {
            out.push((m.default_factory)());
        }
    }

    Ok(out)
}

/// Applies a config-level `hook:` to any rule without each rule's factory
/// having to parse it. Everything else delegates to the wrapped rule.
struct HookOverride {
    inner: Box<dyn Rule>,
    hook: Hook,
}

impl Rule for HookOverride {
    fn id(&self) -> &'static str {
        self.inner.id()
    }
    fn description(&self) -> &'static str {
        self.inner.description()
    }
    fn severity(&self) -> crate::finding::Severity {
        self.inner.severity()
    }
    fn supports_autofix(&self) -> bool {
        self.inner.supports_autofix()
    }
    fn autofix_enabled(&self) -> bool {
        self.inner.autofix_enabled()
    }
    fn hook(&self) -> Hook {
        self.hook
    }
    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        self.inner.check(ctx)
    }
    fn autofix(&self, source: &str) -> String {
        self.inner.autofix(source)
    }
}
