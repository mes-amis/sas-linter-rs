use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::{Token, TokenStream};

use super::RuleMeta;

pub struct MissingAssignmentSemicolon {
    autofix: bool,
}

const ID: &str = "missing_assignment_semicolon";
const DESCRIPTION: &str = "Assignment missing terminating `;` — the inline \
                            `**` / `*` comment marker was lexed as an operator.";

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
        let mut findings = Vec::new();
        let lines: Vec<&str> = ctx.source.split('\n').collect();
        for (t, prev, next) in star2_offenders(&ctx.tokens.default, &lines) {
            findings.push(Finding {
                path: ctx.path.to_string(),
                line: t.start_line,
                column: t.start_column + 1,
                rule: ID,
                message: format!(
                    "`**` parsed as exponentiation in `{} ** {}` — looks like a \
                     missing `;` before an inline `** ... ;` comment.",
                    prev.text, next.text
                ),
                severity: Severity::Warning,
            });
        }
        for (t, prev, next) in star_offenders(&ctx.tokens.default) {
            findings.push(Finding {
                path: ctx.path.to_string(),
                line: t.start_line,
                column: t.start_column + 1,
                rule: ID,
                message: format!(
                    "`*` parsed as multiplication in `{} * {} …` — looks like a \
                     missing `;` before an inline `* ... ;` comment.",
                    prev.text, next.text
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
        let mut lines: Vec<String> = source.split('\n').map(|s| s.to_string()).collect();
        let line_refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let mut edits: Vec<(usize, u32)> = star2_offenders(&stream.default, &line_refs)
            .into_iter()
            .map(|(_, prev, _)| {
                (
                    (prev.start_line as usize).saturating_sub(1),
                    prev.end_column,
                )
            })
            .collect();
        // Apply within a line right-to-left.
        edits.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));

        for (line_idx, col) in edits {
            let Some(line) = lines.get_mut(line_idx) else {
                continue;
            };
            let chars: Vec<char> = line.chars().collect();
            let col = col as usize;
            if col > chars.len() {
                continue;
            }
            let before: String = chars[..col].iter().collect();
            let after: String = chars[col..].iter().collect();
            // Patterns from Ruby:
            //   two+ spaces between IDENT and `**`: consume one for `;`
            //   exactly one space: keep it after the `;`
            //   no space: inject `; `
            let replacement = if let Some(rest) = after.strip_prefix("  ") {
                format!("; {}", rest)
            } else if let Some(rest) = after.strip_prefix(' ') {
                format!("; {}", rest)
            } else {
                format!("; {}", after)
            };
            *line = format!("{}{}", before, replacement);
        }
        insert_star_semicolons(&lines.join("\n"))
    }
}

/// Words that may legitimately follow an operand inside an expression, so a
/// pair of bare words containing one isn't comment prose.
const OPERATOR_WORDS: &[&str] = &[
    "and", "or", "not", "eq", "ne", "lt", "le", "gt", "ge", "in", "min", "max", "then", "else",
    "to", "by", "while", "until",
];

fn is_bare_word(t: &Token) -> bool {
    let mut chars = t.text.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !OPERATOR_WORDS.contains(&t.text.to_ascii_lowercase().as_str())
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

/// `*` in an assignment followed by two bare words with no operator between
/// them (`x = x * Some description;`): the text after `*` is a comment, not
/// an operand.
fn star_offenders(tokens: &[Token]) -> Vec<(&Token, &Token, &Token)> {
    let mut out = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        if t.token_type != TokenType::STAR || i == 0 {
            continue;
        }
        let (Some(next), Some(after)) = (tokens.get(i + 1), tokens.get(i + 2)) else {
            continue;
        };
        if is_bare_word(next) && is_bare_word(after) && in_assignment(tokens, i) {
            out.push((t, &tokens[i - 1], next));
        }
    }
    out
}

/// Terminate each offending assignment straight after the operand that
/// precedes its `*`, turning the rest into a `* … ;` comment statement.
fn insert_star_semicolons(source: &str) -> String {
    let stream = TokenStream::tokenize(source);
    let mut ends: Vec<usize> = star_offenders(&stream.default)
        .into_iter()
        .map(|(_, prev, _)| prev.end_byte as usize)
        .collect();
    ends.sort_unstable_by(|a, b| b.cmp(a));
    let mut out = source.to_string();
    for end in ends {
        out.insert(end, ';');
    }
    out
}

fn star2_offenders<'a>(
    tokens: &'a [Token],
    lines: &[&str],
) -> Vec<(&'a Token, &'a Token, &'a Token)> {
    let mut out = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        if t.token_type != TokenType::STAR2 {
            continue;
        }
        let Some(line) = lines.get((t.start_line as usize).saturating_sub(1)) else {
            continue;
        };
        if line.trim_start().starts_with("**") {
            continue;
        }
        let (Some(prev), Some(next)) = (
            i.checked_sub(1).and_then(|p| tokens.get(p)),
            tokens.get(i + 1),
        ) else {
            continue;
        };
        if prev.token_type != TokenType::Identifier {
            continue;
        }
        if next.token_type != TokenType::Identifier {
            continue;
        }
        out.push((t, prev, next));
    }
    out
}
