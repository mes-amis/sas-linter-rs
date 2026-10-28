use std::collections::HashSet;

use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::Token;

use super::condition::{self, CmpOp, Lit};
use super::RuleMeta;

pub struct UnreachableInnerBranchValue;

const ID: &str = "unreachable_inner_branch_value";
const DESCRIPTION: &str = "Inner branch tests a variable against a value that the \
                            enclosing `if VAR in (...) then do;` guard excludes — the \
                            branch can never fire for that value.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: false,
        default_factory: || Box::new(UnreachableInnerBranchValue),
        config_factory: |_| Ok(Box::new(UnreachableInnerBranchValue)),
    }
}

#[derive(Debug)]
struct GuardFrame {
    var: String,
    allowed: HashSet<Lit>,
    depth: i32,
    line: u32,
    /// The guard's test as written (`x = 4`, `e in (0,1,2)`), for messages.
    text: String,
}

impl Rule for UnreachableInnerBranchValue {
    fn id(&self) -> &'static str {
        ID
    }
    fn description(&self) -> &'static str {
        DESCRIPTION
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        let tokens = &ctx.tokens.default;
        let mut findings = Vec::new();
        let mut stack: Vec<GuardFrame> = Vec::new();
        let mut do_depth: i32 = 0;
        let mut i = 0;

        while i < tokens.len() {
            let tok = &tokens[i];
            match tok.token_type {
                TokenType::KwIf => {
                    let (consumed, frames, inner) =
                        analyze_if(tokens, i, do_depth, &stack, ctx.path, ctx.source);
                    findings.extend(inner);
                    // Frames take effect inside the `do;` that follows the
                    // `then`; the `do` itself is left for the arm below.
                    stack.extend(frames);
                    i += consumed;
                    continue;
                }
                TokenType::KwDo => {
                    do_depth += 1;
                    i += 1;
                    continue;
                }
                TokenType::KwEnd => {
                    if do_depth > 0 {
                        do_depth -= 1;
                    }
                    while let Some(last) = stack.last() {
                        if last.depth > do_depth {
                            stack.pop();
                        } else {
                            break;
                        }
                    }
                    i += 1;
                    continue;
                }
                _ => {
                    i += 1;
                }
            }
        }
        findings
    }
}

/// Analyse the `if` at `i`. Returns how many tokens to skip (the condition,
/// up to but not including `then`), the guard frames the statement opens if
/// it is a `then do;` block, and the findings for values the enclosing
/// guards exclude.
fn analyze_if(
    tokens: &[Token],
    i: usize,
    do_depth: i32,
    stack: &[GuardFrame],
    path: &str,
    source: &str,
) -> (usize, Vec<GuardFrame>, Vec<Finding>) {
    let Some(then_idx) = find_then(tokens, i + 1) else {
        return (1, vec![], vec![]);
    };
    let cond = &tokens[i + 1..then_idx];
    let is_guard = tokens.get(then_idx + 1).map(|t| t.token_type) == Some(TokenType::KwDo)
        && tokens.get(then_idx + 2).map(|t| t.token_type) == Some(TokenType::SEMI);

    let mut findings = Vec::new();
    let mut frames = Vec::new();
    for cmp in conjunct_comparisons(cond) {
        let var = cmp.var_key();
        if let Some(frame) = stack.iter().rev().find(|f| f.var == var) {
            let dead: Vec<&condition::Value> = cmp
                .values
                .iter()
                .filter(|v| !frame.allowed.contains(&v.lit))
                .collect();
            // Every value excluded: the test can never be true, so the
            // whole branch is dead. Some excluded: the branch still fires
            // for the live values, only the dead ones are noise.
            let verdict = if dead.len() == cmp.values.len() {
                "this branch is unreachable."
            } else {
                "this branch can never fire for that value."
            };
            for val in dead {
                findings.push(Finding {
                    path: path.to_string(),
                    line: val.tok.start_line,
                    column: val.tok.start_column + 1,
                    rule: ID,
                    message: format!(
                        "value {} for {} is excluded by the enclosing \
                         `if {}` guard at line {}; {}",
                        val.display, cmp.var.text, frame.text, frame.line, verdict
                    ),
                    severity: Severity::Warning,
                });
            }
        }
        if is_guard {
            frames.push(GuardFrame {
                var,
                allowed: cmp.values.iter().map(|v| v.lit.clone()).collect(),
                depth: do_depth + 1,
                line: tokens[i].start_line,
                text: cmp.source_text(source),
            });
        }
    }
    (then_idx - i, frames, findings)
}

/// Index of the `then` closing the condition that starts at `i`, or `None`
/// if a `;` (or EOF) comes first.
fn find_then(tokens: &[Token], i: usize) -> Option<usize> {
    tokens[i..]
        .iter()
        .position(|t| matches!(t.token_type, TokenType::KwThen | TokenType::SEMI))
        .map(|p| i + p)
        .filter(|&j| tokens[j].token_type == TokenType::KwThen)
}

/// The `VAR in (…)` / `VAR = lit` tests in a condition, one per top-level
/// `and` conjunct. Conjuncts of any other shape (`not`, `ne`, ranges,
/// function calls, …) are skipped; a top-level disjunction yields nothing:
/// a dead value in one disjunct doesn't make the branch unreachable, and as
/// a guard it doesn't restrict anything.
fn conjunct_comparisons(cond: &[Token]) -> Vec<condition::Comparison<'_>> {
    condition::split_conjuncts(cond)
        .unwrap_or_default()
        .into_iter()
        .filter_map(condition::comparison)
        .filter(|cmp| cmp.op == CmpOp::In)
        .collect()
}
