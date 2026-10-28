use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::{Token, TokenStream};

use super::sas::FORMAT_DEFINITIONS;
use super::{statement_not_valid, RuleMeta};

pub struct MissingAssignmentSemicolon {
    autofix: bool,
}

const ID: &str = "missing_assignment_semicolon";
const DESCRIPTION: &str = "Assignment (or other statement) missing its terminating \
                            `;` — the `**` / `*` comment after it was lexed as an \
                            operator.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: true,
        default_factory: || Box::new(MissingAssignmentSemicolon { autofix: false }),
        config_factory: |opts| {
            Ok(Box::new(MissingAssignmentSemicolon {
                autofix: crate::config::opt_bool(opts, "autofix").unwrap_or(false),
            }))
        },
    }
}

impl Rule for MissingAssignmentSemicolon {
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
        offenders(&ctx.tokens.default)
            .into_iter()
            .map(|(t, prev, next)| {
                let (op, reads_as) = if t.token_type == TokenType::STAR2 {
                    ("**", "exponentiation")
                } else {
                    ("*", "multiplication")
                };
                Finding {
                    path: ctx.path.to_string(),
                    line: t.start_line,
                    column: t.start_column + 1,
                    rule: ID,
                    message: format!(
                        "`{op}` parsed as {reads_as} in `{} {op} {} …` — looks like a \
                         missing `;` before an inline `{op} ... ;` comment.",
                        prev.text, next.text
                    ),
                    severity: Severity::Warning,
                }
            })
            .collect()
    }

    /// Terminate each offending assignment straight after the operand that
    /// precedes its `*` / `**`, turning the rest into a comment statement.
    /// Where two or more blanks precede the marker, the `;` takes one of
    /// them so the comment keeps its column.
    fn autofix(&self, source: &str) -> String {
        let stream = TokenStream::tokenize(source);
        let mut ends: Vec<usize> = offenders(&stream.default)
            .into_iter()
            .map(|(_, prev, _)| prev.end_byte as usize)
            .collect();
        ends.sort_unstable_by(|a, b| b.cmp(a));
        let mut out = source.to_string();
        for end in ends {
            let spare_blank = out[end..].starts_with("  ");
            out.replace_range(end..end + usize::from(spare_blank), ";");
        }
        out
    }
}

/// Words that act as operators inside an expression. `in` only does when
/// a `(` list follows it.
const OPERATOR_WORDS: &[&str] = &[
    "and", "or", "not", "eq", "ne", "lt", "le", "gt", "ge", "min", "max",
];

/// An identifier-shaped token at `tokens[i]` that isn't acting as an
/// operator — an operand, or a word of comment prose.
fn is_bare_word(tokens: &[Token], i: usize) -> bool {
    let t = &tokens[i];
    let mut chars = t.text.chars();
    let word_shaped = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    let lower = t.text.to_ascii_lowercase();
    let operator = OPERATOR_WORDS.contains(&lower.as_str())
        || (lower == "in" && tokens.get(i + 1).map(|n| n.token_type) == Some(TokenType::LPAREN));
    word_shaped && !operator
}

/// Whether `tokens[i]` is the first token on its line.
fn begins_line(tokens: &[Token], i: usize) -> bool {
    i > 0 && tokens[i - 1].end_line < tokens[i].start_line
}

/// Whether the statement holding `tokens[i]` is an assignment — `VAR = …`
/// at the start of a statement or right after `then` / `else`.
fn in_assignment(tokens: &[Token], i: usize) -> bool {
    let start = tokens[..i]
        .iter()
        .rposition(|t| t.token_type == TokenType::SEMI)
        .map_or(0, |p| p + 1);
    (start..i).any(|k| {
        tokens[k].token_type == TokenType::ASSIGN
            && k > start
            && tokens[k - 1].token_type == TokenType::Identifier
            && (k - 1 == start
                || matches!(
                    tokens[k - 2].token_type,
                    TokenType::KwThen | TokenType::KwElse
                ))
    })
}

/// `*` / `**` markers that start comment prose, one per statement: in an
/// assignment, or at the start of a line inside a recognisable statement
/// whose `;` is missing (a `** … **;` banner swallowed by the `value`
/// statement above it). Mid-line outside an assignment, words can follow
/// `*` legitimately (`tables a * b c;`). A statement that is prose from its
/// first word is leaked header text, which `source_headers` reports.
fn offenders(tokens: &[Token]) -> Vec<(&Token, &Token, &Token)> {
    let mut out = Vec::new();
    let mut stmt_start = 0;
    let mut marker: Option<usize> = None;
    let mut reported = false;
    for k in 0..tokens.len() {
        match tokens[k].token_type {
            TokenType::SEMI => {
                stmt_start = k + 1;
                marker = None;
                reported = false;
                continue;
            }
            TokenType::STAR | TokenType::STAR2 if k > 0 => marker = Some(k),
            _ => {}
        }
        let Some(m) = marker else {
            continue;
        };
        if !reported
            && k > m
            && k + 1 < tokens.len()
            && is_bare_word(tokens, k)
            && is_bare_word(tokens, k + 1)
            && (in_assignment(tokens, m)
                || (begins_line(tokens, m) && is_real_statement(&tokens[stmt_start..m])))
        {
            out.push((&tokens[m], &tokens[m - 1], &tokens[m + 1]));
            reported = true;
        }
    }
    out
}

/// A statement that starts like SAS code: a data-step or global statement,
/// or a PROC FORMAT definition.
fn is_real_statement(stmt: &[Token]) -> bool {
    stmt.first()
        .is_some_and(|first| FORMAT_DEFINITIONS.contains(&first.text.to_lowercase().as_str()))
        || (!stmt.is_empty() && statement_not_valid::starts_validly(stmt))
}
