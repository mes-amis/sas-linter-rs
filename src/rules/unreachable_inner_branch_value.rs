use std::collections::HashSet;

use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::Token;

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
                    let (consumed, frames, inner) =
                        analyze_if(tokens, i, do_depth, &stack, ctx.path);
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

/// Operators that make a condition a disjunction. `and` binds tighter than
/// `or`, so once one of these appears at the top level the condition can no
/// longer be read as a set of independent tests: a dead value in one
/// disjunct doesn't make the branch unreachable, and as a guard it doesn't
/// restrict anything.
const DISJUNCTION_OPS: &[TokenType] = &[
    TokenType::KwOR,
    TokenType::PIPE,
    TokenType::PIPE2,
    TokenType::EXCL,
    TokenType::EXCL2,
    TokenType::AMP,
];

/// One `VAR in (…)` / `VAR = lit` test lifted out of a condition.
struct Comparison {
    var: String,
    display: String,
    values: Vec<LitValue>,
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
        if let Some(frame) = stack.iter().rev().find(|f| f.var == cmp.var) {
            let dead: Vec<&LitValue> = cmp
                .values
                .iter()
                .filter(|v| !frame.allowed.contains(&v.key))
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
                    line: val.line,
                    column: val.column,
                    rule: ID,
                    message: format!(
                        "value {} for {} is excluded by the enclosing \
                         `if {} in (...)` guard at line {}; {}",
                        val.display, cmp.display, cmp.display, frame.line, verdict
                    ),
                    severity: Severity::Warning,
                });
            }
        }
        if is_guard {
            frames.push(GuardFrame {
                var: cmp.var,
                allowed: cmp.values.iter().map(|v| v.key.clone()).collect(),
                depth: do_depth + 1,
                line: tokens[i].start_line,
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
/// function calls, …) are skipped; a top-level disjunction yields nothing.
fn conjunct_comparisons(cond: &[Token]) -> Vec<Comparison> {
    let cond = strip_outer_parens(cond);
    let mut parts: Vec<&[Token]> = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (j, t) in cond.iter().enumerate() {
        match t.token_type {
            TokenType::LPAREN => depth += 1,
            TokenType::RPAREN => depth = depth.saturating_sub(1),
            TokenType::KwAND if depth == 0 => {
                parts.push(&cond[start..j]);
                start = j + 1;
            }
            ty if depth == 0 && DISJUNCTION_OPS.contains(&ty) => return Vec::new(),
            _ => {}
        }
    }
    parts.push(&cond[start..]);

    parts
        .into_iter()
        .filter_map(|part| {
            let part = strip_outer_parens(part);
            let ident = part
                .first()
                .filter(|t| t.token_type == TokenType::Identifier)?;
            let op = part.get(1)?;
            let (values, end) = parse_comparison(part, 1, op)?;
            // The comparison must be the whole conjunct — `e in (9) + 1`
            // or `e = 1 - x` is arithmetic, not a set test.
            (end == part.len()).then(|| Comparison {
                var: ident.text.to_lowercase(),
                display: ident.text.clone(),
                values,
            })
        })
        .collect()
}

/// Peel `( … )` pairs that enclose the entire slice.
fn strip_outer_parens(mut toks: &[Token]) -> &[Token] {
    loop {
        let n = toks.len();
        if n < 2
            || toks[0].token_type != TokenType::LPAREN
            || toks[n - 1].token_type != TokenType::RPAREN
        {
            return toks;
        }
        let mut depth = 0usize;
        for (j, t) in toks.iter().enumerate() {
            match t.token_type {
                TokenType::LPAREN => depth += 1,
                TokenType::RPAREN => {
                    depth -= 1;
                    if depth == 0 && j != n - 1 {
                        return toks;
                    }
                }
                _ => {}
            }
        }
        toks = &toks[1..n - 1];
    }
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
                } else {
                    let v = literal_value(t)?;
                    values.push(v);
                    k += 1;
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
