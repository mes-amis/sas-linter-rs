use std::collections::HashSet;

use sas_lexer::TokenType;

// HashSet is for *consumed_thens token indices*, not for token types.
// (Indices are usize and trivially hashable.)

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::Token;

use super::RuleMeta;

pub struct MalformedIfCondition;

const ID: &str = "malformed_if_condition";
const DESCRIPTION: &str = "Validate `if ... then` conditions form a well-shaped \
                            boolean expression (no missing operators, operands, \
                            or `if` keyword; balanced parens).";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: false,
        default_factory: || Box::new(MalformedIfCondition),
        config_factory: |_| Ok(Box::new(MalformedIfCondition)),
    }
}

// sas-lexer's TokenType doesn't implement Hash, so we use linear scans
// over flat slices. The sets are small (≤25 entries) so this is fine.
const COMPARISON_OPS: &[TokenType] = &[
    TokenType::ASSIGN,
    TokenType::KwEQ,
    TokenType::KwNE,
    TokenType::NE,
    TokenType::KwLT,
    TokenType::LT,
    TokenType::KwLE,
    TokenType::LE,
    TokenType::KwGT,
    TokenType::GT,
    TokenType::KwGE,
    TokenType::GE,
    TokenType::KwIN,
    TokenType::SoundsLike,
    TokenType::GTLT,
    TokenType::LTGT,
    TokenType::KwEQT,
    TokenType::KwGTT,
    TokenType::KwLTT,
    TokenType::KwGET,
    TokenType::KwLET,
    TokenType::KwNET,
];

const LOGICAL_OPS: &[TokenType] = &[
    TokenType::KwAND,
    TokenType::KwOR,
    TokenType::AMP,
    TokenType::PIPE,
    TokenType::PIPE2,
];

const ARITH_OPS: &[TokenType] = &[
    TokenType::PLUS,
    TokenType::MINUS,
    TokenType::STAR,
    TokenType::FSLASH,
    TokenType::STAR2,
    TokenType::EXCL,
    TokenType::EXCL2,
    TokenType::BPIPE,
    TokenType::BPIPE2,
];

const UNARY_PREFIXES: &[TokenType] = &[
    TokenType::KwNOT,
    TokenType::NOT,
    TokenType::MINUS,
    TokenType::PLUS,
];

const OPERAND_TOKENS: &[TokenType] = &[
    TokenType::Identifier,
    TokenType::IntegerLiteral,
    TokenType::FloatLiteral,
    TokenType::FloatExponentLiteral,
    TokenType::StringLiteral,
    TokenType::HexStringLiteral,
    TokenType::BitTestingLiteral,
    TokenType::DateLiteral,
    TokenType::DateTimeLiteral,
    TokenType::TimeLiteral,
    TokenType::NameLiteral,
    TokenType::MacroVarResolve,
    TokenType::MacroIdentifier,
    TokenType::MacroString,
    TokenType::StringExprStart,
];

fn is_binop(t: TokenType) -> bool {
    COMPARISON_OPS.contains(&t) || LOGICAL_OPS.contains(&t) || ARITH_OPS.contains(&t)
}

#[derive(PartialEq)]
enum State {
    ExpectOperand,
    ExpectOperator,
}

impl Rule for MalformedIfCondition {
    fn id(&self) -> &'static str {
        ID
    }
    fn description(&self) -> &'static str {
        DESCRIPTION
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        let tokens = &ctx.tokens.default;
        let mut findings = Vec::new();
        let mut consumed_thens: HashSet<usize> = HashSet::new();
        let mut i = 0;

        while i < tokens.len() {
            if tokens[i].token_type == TokenType::KwIf {
                let (new_i, sub) = analyze_if(tokens, i, ctx.path, &mut consumed_thens);
                findings.extend(sub);
                i = new_i;
                continue;
            }
            if tokens[i].token_type == TokenType::KwThen && !consumed_thens.contains(&i) {
                let t = &tokens[i];
                findings.push(Finding {
                    path: ctx.path.to_string(),
                    line: t.start_line,
                    column: t.start_column + 1,
                    rule: ID,
                    message: "`then` without a preceding `if` condition — likely missing `if`."
                        .into(),
                    severity: Severity::Warning,
                });
            }
            i += 1;
        }
        findings
    }
}

fn analyze_if(
    tokens: &[Token],
    start: usize,
    path: &str,
    consumed: &mut HashSet<usize>,
) -> (usize, Vec<Finding>) {
    let mut findings = Vec::new();
    let mut state = State::ExpectOperand;
    let mut paren_depth: i32 = 0;
    let mut open_parens: Vec<&Token> = Vec::new();
    let mut cond_started = false;
    let mut last_op_tok: Option<&Token> = None;
    let mut broken = false;

    let mut i = start + 1;
    while i < tokens.len() {
        let t = &tokens[i];
        let ty = t.token_type;

        if broken {
            if ty == TokenType::KwThen {
                consumed.insert(i);
            }
            if ty == TokenType::SEMI {
                return (i + 1, findings);
            }
            i += 1;
            continue;
        }

        if ty == TokenType::KwThen && paren_depth == 0 {
            flag_terminal(
                &mut findings,
                path,
                &state,
                cond_started,
                last_op_tok,
                t,
                "then",
            );
            consumed.insert(i);
            return (i + 1, findings);
        }
        if ty == TokenType::SEMI && paren_depth == 0 {
            flag_terminal(
                &mut findings,
                path,
                &state,
                cond_started,
                last_op_tok,
                t,
                "subsetting `if`",
            );
            return (i + 1, findings);
        }
        if (ty == TokenType::KwThen || ty == TokenType::SEMI) && paren_depth > 0 {
            let lp = open_parens.first().copied().unwrap_or(t);
            findings.push(Finding {
                path: path.to_string(),
                line: lp.start_line,
                column: lp.start_column + 1,
                rule: ID,
                message: format!(
                    "unbalanced `(` in `if` condition (no matching `)` before `{}`).",
                    t.text
                ),
                severity: Severity::Warning,
            });
            broken = true;
            if ty == TokenType::KwThen {
                consumed.insert(i);
            }
            i += 1;
            continue;
        }

        if matches!(ty, TokenType::LPAREN | TokenType::LBRACK) {
            cond_started = true;
            paren_depth += 1;
            open_parens.push(t);
            i += 1;
            continue;
        }
        if matches!(ty, TokenType::RPAREN | TokenType::RBRACK) {
            if paren_depth == 0 {
                findings.push(Finding {
                    path: path.to_string(),
                    line: t.start_line,
                    column: t.start_column + 1,
                    rule: ID,
                    message: format!("unbalanced `{}` in `if` condition.", t.text),
                    severity: Severity::Warning,
                });
                broken = true;
                i += 1;
                continue;
            }
            paren_depth -= 1;
            open_parens.pop();
            if paren_depth == 0 {
                state = State::ExpectOperator;
            }
            i += 1;
            continue;
        }
        if paren_depth > 0 {
            i += 1;
            continue;
        }
        if ty == TokenType::COMMA {
            i += 1;
            continue;
        }
        cond_started = true;

        if state == State::ExpectOperand {
            if UNARY_PREFIXES.contains(&ty) {
                i += 1;
                continue;
            }
            if OPERAND_TOKENS.contains(&ty) {
                state = State::ExpectOperator;
                i += 1;
                continue;
            }
            if is_binop(ty) {
                let msg = match last_op_tok {
                    None => format!(
                        "operator `{}` at start of `if` condition with no left operand.",
                        t.text
                    ),
                    Some(prev) => format!(
                        "operator `{}` follows operator `{}` with no operand between them.",
                        t.text, prev.text
                    ),
                };
                findings.push(Finding {
                    path: path.to_string(),
                    line: t.start_line,
                    column: t.start_column + 1,
                    rule: ID,
                    message: msg,
                    severity: Severity::Warning,
                });
                broken = true;
                last_op_tok = Some(t);
                i += 1;
                continue;
            }
            // Unknown token in operand position — treat opaquely.
            state = State::ExpectOperator;
            i += 1;
            continue;
        }

        // State::ExpectOperator
        if is_binop(ty) {
            last_op_tok = Some(t);
            state = State::ExpectOperand;
            i += 1;
            continue;
        }
        // Negated comparisons: `not eq`, `^=`, etc.
        if matches!(ty, TokenType::KwNOT | TokenType::NOT) {
            if let Some(nxt) = tokens.get(i + 1) {
                if COMPARISON_OPS.contains(&nxt.token_type) {
                    last_op_tok = Some(nxt);
                    state = State::ExpectOperand;
                    i += 2;
                    continue;
                }
            }
        }

        if OPERAND_TOKENS.contains(&ty) || UNARY_PREFIXES.contains(&ty) {
            findings.push(Finding {
                path: path.to_string(),
                line: t.start_line,
                column: t.start_column + 1,
                rule: ID,
                message: format!(
                    "missing operator before `{}` in `if` condition — \
                     perhaps a missing `and`/`or`?",
                    t.text
                ),
                severity: Severity::Warning,
            });
            broken = true;
            i += 1;
            continue;
        }
        i += 1;
    }

    (i, findings)
}

fn flag_terminal(
    findings: &mut Vec<Finding>,
    path: &str,
    state: &State,
    cond_started: bool,
    last_op_tok: Option<&Token>,
    terminator: &Token,
    where_: &str,
) {
    if !cond_started {
        findings.push(Finding {
            path: path.to_string(),
            line: terminator.start_line,
            column: terminator.start_column + 1,
            rule: ID,
            message: format!("`if {}` with empty condition.", where_),
            severity: Severity::Warning,
        });
    } else if *state == State::ExpectOperand {
        if let Some(prev) = last_op_tok {
            findings.push(Finding {
                path: path.to_string(),
                line: prev.start_line,
                column: prev.start_column + 1,
                rule: ID,
                message: format!(
                    "operator `{}` has no right operand before `{}`.",
                    prev.text, terminator.text
                ),
                severity: Severity::Warning,
            });
        }
    }
}
