use std::collections::HashSet;

use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::Token;

use super::RuleMeta;

pub struct UnreachableInnerBranchValue;

const ID: &str = "unreachable_inner_branch_value";
const DESCRIPTION: &str = "Inner branch references a value that the enclosing \
                            outer guard excludes — branch is unreachable for that value.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: false,
        default_factory: || Box::new(UnreachableInnerBranchValue),
        config_factory: |_| Ok(Box::new(UnreachableInnerBranchValue)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum LitKey {
    Int(i64),
    Float(u64), // bit pattern
    Str(String),
}

#[derive(Debug, Clone)]
struct LitValue {
    key: LitKey,
    display: String,
    line: u32,
    column: u32,
}

#[derive(Debug)]
struct GuardFrame {
    var: String,
    allowed: HashSet<LitKey>,
    depth: i32,
    line: u32,
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
                    let (consumed, frame, inner) =
                        analyze_if(tokens, i, do_depth, &stack, ctx.path);
                    findings.extend(inner);
                    if let Some(f) = frame {
                        stack.push(f);
                        do_depth += 1;
                    }
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

fn analyze_if(
    tokens: &[Token],
    i: usize,
    do_depth: i32,
    stack: &[GuardFrame],
    path: &str,
) -> (usize, Option<GuardFrame>, Vec<Finding>) {
    let Some(ident) = tokens.get(i + 1) else {
        return (1, None, vec![]);
    };
    if ident.token_type != TokenType::Identifier {
        return (1, None, vec![]);
    }
    let var = ident.text.to_lowercase();
    let Some(op) = tokens.get(i + 2) else {
        return (1, None, vec![]);
    };

    let Some((values, end_of_cond)) = parse_comparison(tokens, i + 2, op) else {
        return (1, None, vec![]);
    };

    let then_pos = end_of_cond;
    let is_outer_guard = tokens.get(then_pos).map(|t| t.token_type) == Some(TokenType::KwThen)
        && tokens.get(then_pos + 1).map(|t| t.token_type) == Some(TokenType::KwDo)
        && tokens.get(then_pos + 2).map(|t| t.token_type) == Some(TokenType::SEMI);

    let mut findings = Vec::new();
    if !is_outer_guard {
        if let Some(frame) = stack.iter().rev().find(|f| f.var == var) {
            for val in &values {
                if !frame.allowed.contains(&val.key) {
                    findings.push(Finding {
                        path: path.to_string(),
                        line: val.line,
                        column: val.column,
                        rule: ID,
                        message: format!(
                            "value {} for {} is excluded by the enclosing \
                             `if {} in (...)` guard at line {}; this branch is unreachable.",
                            val.display, ident.text, ident.text, frame.line
                        ),
                        severity: Severity::Warning,
                    });
                }
            }
        }
    }

    let mut new_frame = None;
    let mut consumed = end_of_cond - i;
    if is_outer_guard {
        new_frame = Some(GuardFrame {
            var,
            allowed: values.iter().map(|v| v.key.clone()).collect(),
            depth: do_depth + 1,
            line: tokens[i].start_line,
        });
        consumed = (then_pos + 3) - i;
    }
    (consumed, new_frame, findings)
}

fn parse_comparison(tokens: &[Token], op_idx: usize, op: &Token) -> Option<(Vec<LitValue>, usize)> {
    match op.token_type {
        TokenType::KwIN => {
            let lparen = tokens.get(op_idx + 1)?;
            if lparen.token_type != TokenType::LPAREN {
                return None;
            }
            let mut values = Vec::new();
            let mut k = op_idx + 2;
            loop {
                let t = tokens.get(k)?;
                if t.token_type == TokenType::RPAREN {
                    return Some((values, k + 1));
                } else if t.token_type == TokenType::COMMA {
                    k += 1;
                } else if let Some(v) = literal_value(t) {
                    values.push(v);
                    k += 1;
                } else {
                    return None;
                }
            }
        }
        TokenType::KwEQ | TokenType::ASSIGN => {
            let lit = tokens.get(op_idx + 1)?;
            let v = literal_value(lit)?;
            Some((vec![v], op_idx + 2))
        }
        _ => None,
    }
}

fn literal_value(t: &Token) -> Option<LitValue> {
    match t.token_type {
        TokenType::IntegerLiteral => {
            let n: i64 = t.text.parse().ok()?;
            Some(LitValue {
                key: LitKey::Int(n),
                display: t.text.clone(),
                line: t.start_line,
                column: t.start_column + 1,
            })
        }
        TokenType::FloatLiteral => {
            let f: f64 = t.text.parse().ok()?;
            let key = if f == f.trunc() && f.is_finite() {
                LitKey::Int(f as i64)
            } else {
                LitKey::Float(f.to_bits())
            };
            Some(LitValue {
                key,
                display: t.text.clone(),
                line: t.start_line,
                column: t.start_column + 1,
            })
        }
        TokenType::StringLiteral => Some(LitValue {
            key: LitKey::Str(t.text.clone()),
            display: t.text.clone(),
            line: t.start_line,
            column: t.start_column + 1,
        }),
        _ => None,
    }
}
