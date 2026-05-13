use std::collections::HashMap;

use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::{Token, TokenStream};

use super::RuleMeta;

pub struct InconsistentVariableCase {
    autofix: bool,
}

const ID: &str = "inconsistent_variable_case";
const DESCRIPTION: &str = "Variable identifiers must use one consistent letter case across the \
     file; mixing `myVar` and `MyVar` is sloppy.";

const FORMAT_DEF_KEYWORDS: &[&str] = &["value", "invalue", "picture"];

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: true,
        default_factory: || Box::new(InconsistentVariableCase { autofix: false }),
        config_factory: |opts| {
            Ok(Box::new(InconsistentVariableCase {
                autofix: crate::config::opt_bool(opts, "autofix").unwrap_or(false),
            }))
        },
    }
}

impl Rule for InconsistentVariableCase {
    fn id(&self) -> &'static str {
        ID
    }
    fn description(&self) -> &'static str {
        DESCRIPTION
    }
    fn supports_autofix(&self) -> bool {
        true
    }
    fn autofix_enabled(&self) -> bool {
        self.autofix
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        let mut findings = Vec::new();
        for (token, canonical) in inconsistent_uses(&ctx.tokens.default) {
            findings.push(Finding {
                path: ctx.path.to_string(),
                line: token.start_line,
                column: token.start_column + 1,
                rule: ID,
                message: format!(
                    "variable `{}` is spelled `{}` elsewhere in this file — \
                     pick one case and stick with it.",
                    token.text, canonical
                ),
                severity: Severity::Warning,
            });
        }
        findings
    }

    fn autofix(&self, source: &str) -> String {
        if source.is_empty() {
            return source.to_string();
        }
        let stream = TokenStream::tokenize(source);
        let mut edits: Vec<(u32, u32, String)> = inconsistent_uses(&stream.default)
            .into_iter()
            .map(|(t, canon)| (t.start_byte, t.end_byte, canon))
            .collect();
        // Apply right-to-left so earlier byte offsets remain valid.
        edits.sort_by_key(|e| std::cmp::Reverse(e.0));

        let mut out = source.to_string();
        for (start, end, repl) in edits {
            let start = start as usize;
            let end = end as usize;
            if start <= out.len() && end <= out.len() && start <= end {
                out.replace_range(start..end, &repl);
            }
        }
        out
    }
}

fn inconsistent_uses(tokens: &[Token]) -> Vec<(Token, String)> {
    let mut groups: HashMap<String, Vec<Token>> = HashMap::new();
    for (i, t) in tokens.iter().enumerate() {
        if t.token_type != TokenType::Identifier {
            continue;
        }
        if !is_variable_use(tokens, i) {
            continue;
        }
        groups
            .entry(t.text.to_lowercase())
            .or_default()
            .push(t.clone());
    }

    let mut out = Vec::new();
    for (_key, uses) in groups {
        if uses.len() < 2 {
            continue;
        }
        let mut counts: HashMap<String, usize> = HashMap::new();
        for u in &uses {
            *counts.entry(u.text.clone()).or_default() += 1;
        }
        if counts.len() <= 1 {
            continue;
        }
        let canonical = canonical_form(&counts, &uses);
        for u in &uses {
            if u.text != canonical {
                out.push((u.clone(), canonical.clone()));
            }
        }
    }
    out
}

fn canonical_form(counts: &HashMap<String, usize>, uses: &[Token]) -> String {
    let max_count = counts.values().copied().max().unwrap_or(0);
    let winners: Vec<&String> = counts
        .iter()
        .filter_map(|(k, v)| if *v == max_count { Some(k) } else { None })
        .collect();
    if winners.len() == 1 {
        return winners[0].clone();
    }
    for u in uses {
        if winners.contains(&&u.text) {
            return u.text.clone();
        }
    }
    winners[0].clone()
}

fn is_variable_use(tokens: &[Token], i: usize) -> bool {
    let t = &tokens[i];
    let nxt = tokens.get(i + 1);
    let prev = i.checked_sub(1).and_then(|p| tokens.get(p));

    if let Some(n) = nxt {
        if n.token_type == TokenType::DOT && n.start_byte == t.end_byte {
            return false;
        }
    }
    if let Some(p) = prev {
        if p.token_type == TokenType::DOT && p.end_byte == t.start_byte {
            return false;
        }
    }
    if FORMAT_DEF_KEYWORDS.contains(&t.text.to_lowercase().as_str()) {
        return false;
    }
    if let Some(p) = prev {
        if p.token_type == TokenType::Identifier
            && FORMAT_DEF_KEYWORDS.contains(&p.text.to_lowercase().as_str())
        {
            return false;
        }
    }
    true
}
