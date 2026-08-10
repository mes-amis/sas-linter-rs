use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::Token;

use super::RuleMeta;

pub struct UnbalancedDoBlock;

const ID: &str = "unbalanced_do_block";
const DESCRIPTION: &str = "`do` / `end` nesting doesn't balance — an unclosed \
                            `do` (reported at the `do` that never closes) or a \
                            stray `end` with no open block.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        // Deliberately report-only: where a missing `end` belongs is a
        // semantic call. Appending one at EOF parses, but silently changes
        // which statements are conditional — a plausible-looking wrong
        // placement is worse than the error.
        supports_autofix: false,
        default_factory: || Box::new(UnbalancedDoBlock),
        config_factory: |_| Ok(Box::new(UnbalancedDoBlock)),
    }
}

/// What put a frame on the stack. Both are closed by a single `end;`.
#[derive(Clone, Copy)]
enum Opener {
    Do,
    Select,
}

impl Opener {
    fn keyword(self) -> &'static str {
        match self {
            Opener::Do => "do",
            Opener::Select => "select",
        }
    }
}

/// Tokens that can precede `do` / `end` when the keyword is being used as an
/// operand rather than as a statement — `x = end;`, `f(do)`. A real block
/// keyword never appears in any of these positions.
const EXPRESSION_PREFIXES: &[TokenType] = &[
    TokenType::ASSIGN,
    TokenType::LPAREN,
    TokenType::LBRACK,
    TokenType::COMMA,
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

impl Rule for UnbalancedDoBlock {
    fn id(&self) -> &'static str {
        ID
    }
    fn description(&self) -> &'static str {
        DESCRIPTION
    }
    fn severity(&self) -> Severity {
        // A hard SAS compile error ("ERROR 117-185: There were N unclosed DO
        // blocks."), and one SAS reports without a line number.
        Severity::Error
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        let tokens = &ctx.tokens.default;
        if !in_scope(tokens) {
            return Vec::new();
        }

        let mut findings = Vec::new();
        let mut stack: Vec<(Opener, &Token)> = Vec::new();

        for (i, t) in tokens.iter().enumerate() {
            if t.token_type == TokenType::KwDo && opens_block(tokens, i) {
                stack.push((Opener::Do, t));
            } else if t.token_type == TokenType::KwSelect && is_data_step_select(tokens, i) {
                stack.push((Opener::Select, t));
            } else if t.token_type == TokenType::KwEnd && closes_block(tokens, i) {
                if stack.pop().is_none() {
                    findings.push(finding(
                        ctx.path,
                        t,
                        "`end` closes nothing — no `do` or `select` block is open here.".into(),
                    ));
                }
            } else if matches!(t.token_type, TokenType::KwRun | TokenType::KwQuit)
                && followed_by_semi(tokens, i)
            {
                // A step boundary ends the data step body: anything still open
                // is unclosed, and an `end;` in the *next* step must not be
                // credited against it.
                drain(&mut findings, ctx.path, &mut stack);
            }
        }

        // Fragments carry no `run;`, so EOF is the terminator.
        drain(&mut findings, ctx.path, &mut stack);

        findings.sort_by_key(|f| (f.line, f.column));
        findings
    }
}

/// Report every still-open block, in source order, and reset the stack.
fn drain(findings: &mut Vec<Finding>, path: &str, stack: &mut Vec<(Opener, &Token)>) {
    for (opener, tok) in stack.iter() {
        findings.push(finding(
            path,
            tok,
            format!("`{}` is never closed by a matching `end`", opener.keyword()),
        ));
    }
    stack.clear();
}

fn finding(path: &str, tok: &Token, message: String) -> Finding {
    Finding {
        path: path.to_string(),
        line: tok.start_line,
        column: tok.start_column + 1,
        rule: ID,
        message,
        severity: Severity::Error,
    }
}

/// Whole-file scope gate. Two constructs make a token-level `do`/`end` count
/// untrustworthy, and for both the issue's preference is to stay silent
/// rather than risk a false positive:
///
/// * `%do` / `%end` — macro blocks are a separate nesting stack, and
///   `%if`-guarded text can legitimately emit unbalanced-looking `do`/`end`.
/// * a non-data-step `select` — i.e. PROC SQL, where `select` opens no block
///   but `case … end` closes one, so `end` no longer maps onto `do`.
fn in_scope(tokens: &[Token]) -> bool {
    !tokens.iter().enumerate().any(|(i, t)| {
        matches!(t.token_type, TokenType::KwmDo | TokenType::KwmEnd)
            || (t.token_type == TokenType::KwSelect && !is_data_step_select(tokens, i))
    })
}

fn prev_default(tokens: &[Token], i: usize) -> Option<TokenType> {
    i.checked_sub(1).map(|p| tokens[p].token_type)
}

fn followed_by_semi(tokens: &[Token], i: usize) -> bool {
    tokens.get(i + 1).map(|t| t.token_type) == Some(TokenType::SEMI)
}

/// `do` opens a block everywhere it appears as a keyword — plain `do;`,
/// iterative `do i = 1 to 10;`, `do while (…)`, `do until (…)`, `do over arr;`
/// — including after `then` / `else` / `when (…)` / `otherwise`, so position
/// can't be constrained further than "not an operand".
fn opens_block(tokens: &[Token], i: usize) -> bool {
    !matches!(prev_default(tokens, i), Some(ty) if EXPRESSION_PREFIXES.contains(&ty))
}

/// A block-closing `end` is always the whole statement: `end;`. Requiring the
/// `;` is what keeps `set b end = eof;` (data-step option) and `if end then …`
/// (a variable named `end`) from being read as block closes.
fn closes_block(tokens: &[Token], i: usize) -> bool {
    followed_by_semi(tokens, i) && opens_block(tokens, i)
}

/// Distinguish the data-step `select` — `select;` or `select (expr);`, closed
/// by `end;` — from the PROC SQL clause, which is followed by a column list.
fn is_data_step_select(tokens: &[Token], i: usize) -> bool {
    match tokens.get(i + 1).map(|t| t.token_type) {
        Some(TokenType::SEMI) => true,
        Some(TokenType::LPAREN) => {
            let mut depth = 0usize;
            for (j, t) in tokens.iter().enumerate().skip(i + 1) {
                match t.token_type {
                    TokenType::LPAREN => depth += 1,
                    TokenType::RPAREN => {
                        depth -= 1;
                        if depth == 0 {
                            return followed_by_semi(tokens, j);
                        }
                    }
                    // `select` clause ran to the end of a statement without
                    // closing its paren — not the data-step form.
                    TokenType::SEMI => return false,
                    _ => {}
                }
            }
            false
        }
        _ => false,
    }
}
