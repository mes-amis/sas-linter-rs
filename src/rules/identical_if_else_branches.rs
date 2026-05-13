use once_cell::sync::Lazy;
use regex::Regex;
use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::Token;

use super::RuleMeta;

pub struct IdenticalIfElseBranches;

const ID: &str = "identical_if_else_branches";
const DESCRIPTION: &str = "`if ... then S; else S;` — THEN and ELSE bodies are identical, \
     so the condition has no effect.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: false,
        default_factory: || Box::new(IdenticalIfElseBranches),
        config_factory: |_| Ok(Box::new(IdenticalIfElseBranches)),
    }
}

impl Rule for IdenticalIfElseBranches {
    fn id(&self) -> &'static str {
        ID
    }
    fn description(&self) -> &'static str {
        DESCRIPTION
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        let tokens = &ctx.tokens.default;
        let mut findings = Vec::new();
        let mut i = 0;
        while i < tokens.len() {
            if tokens[i].token_type == TokenType::KwThen {
                // Skip `then do; ...` block forms.
                let next = tokens.get(i + 1);
                if next.map(|t| t.token_type) != Some(TokenType::KwDo) {
                    let (then_body, after_then) = collect_simple_body(tokens, i + 1);
                    if let Some(then_body) = then_body {
                        if tokens.get(after_then).map(|t| t.token_type) == Some(TokenType::KwElse) {
                            let else_idx = after_then;
                            let else_first = tokens.get(else_idx + 1);
                            if else_first.map(|t| t.token_type) != Some(TokenType::KwDo) {
                                let (else_body, after_else) =
                                    collect_simple_body(tokens, else_idx + 1);
                                if let Some(else_body) = else_body {
                                    if bodies_equivalent(&then_body, &else_body) {
                                        let else_tok = &tokens[else_idx];
                                        findings.push(Finding {
                                            path: ctx.path.to_string(),
                                            line: else_tok.start_line,
                                            column: else_tok.start_column + 1,
                                            rule: ID,
                                            message: format!(
                                                "`if ... then {}; else {};` — branches are \
                                                 identical; the condition has no effect.",
                                                render_body(&then_body),
                                                render_body(&else_body)
                                            ),
                                            severity: Severity::Warning,
                                        });
                                        i = after_else;
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            i += 1;
        }
        findings
    }
}

/// Collect tokens for one simple statement body starting at `start`, up
/// to (but not including) the terminating SEMI. Returns
/// `(Some(body), index_after_semi)` or `(None, start)` if no SEMI is
/// found before EOF.
fn collect_simple_body(tokens: &[Token], start: usize) -> (Option<Vec<Token>>, usize) {
    let mut body = Vec::new();
    let mut k = start;
    while k < tokens.len() {
        if tokens[k].token_type == TokenType::SEMI {
            return (Some(body), k + 1);
        }
        body.push(tokens[k].clone());
        k += 1;
    }
    (None, start)
}

fn bodies_equivalent(a: &[Token], b: &[Token]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).all(|(ta, tb)| {
        ta.token_type == tb.token_type && ta.text.to_lowercase() == tb.text.to_lowercase()
    })
}

static RE_TIGHT_BEFORE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+([,;()])").unwrap());
static RE_TIGHT_AFTER: Lazy<Regex> = Lazy::new(|| Regex::new(r"([,(])\s+").unwrap());

fn render_body(body: &[Token]) -> String {
    let joined: String = body
        .iter()
        .map(|t| t.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let step1 = RE_TIGHT_BEFORE.replace_all(&joined, "$1");
    let step2 = RE_TIGHT_AFTER.replace_all(&step1, "$1");
    step2.into_owned()
}
