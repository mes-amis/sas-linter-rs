use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::{Token, TokenStream};

use super::RuleMeta;

pub struct MalformedLabelStatement {
    autofix: bool,
}

const ID: &str = "malformed_label_statement";
const DESCRIPTION: &str = "`label` statement missing `=` between variable and string literal, \
                            or missing its terminating `;` so it swallows the next statement.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: true,
        default_factory: || Box::new(MalformedLabelStatement { autofix: false }),
        config_factory: |opts| {
            Ok(Box::new(MalformedLabelStatement {
                autofix: crate::config::opt_bool(opts, "autofix").unwrap_or(false),
            }))
        },
    }
}

impl Rule for MalformedLabelStatement {
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
        for (ident, string) in label_violations(&ctx.tokens.default) {
            findings.push(Finding {
                path: ctx.path.to_string(),
                line: ident.start_line,
                column: ident.start_column + 1,
                rule: ID,
                message: format!(
                    "`label {} {}` is missing the `=` between the variable name and the label string.",
                    ident.text,
                    shorten(&string.text)
                ),
                severity: Severity::Warning,
            });
        }
        for (_, keyword) in unterminated_labels(&ctx.tokens.default) {
            findings.push(Finding {
                path: ctx.path.to_string(),
                line: keyword.start_line,
                column: keyword.start_column + 1,
                rule: ID,
                message: format!(
                    "`label` statement is missing its terminating `;` before `{}` — \
                     the `{}` statement is read as part of the label.",
                    keyword.text, keyword.text
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
        let source = &insert_label_semicolons(source);
        let stream = TokenStream::tokenize(source);
        let mut lines: Vec<String> = source.split('\n').map(|s| s.to_string()).collect();

        // Group by line index then apply right-to-left so earlier
        // columns stay valid.
        use std::collections::BTreeMap;
        let mut by_line: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for (ident, _) in label_violations(&stream.default) {
            by_line
                .entry(ident.start_line - 1)
                .or_default()
                .push(ident.end_column);
        }

        for (line_idx, mut cols) in by_line {
            let Some(line) = lines.get_mut(line_idx as usize) else {
                continue;
            };
            cols.sort();
            cols.reverse();
            for col in cols {
                let col = col as usize;
                let chars: Vec<char> = line.chars().collect();
                if col > chars.len() {
                    continue;
                }
                // Insert ` =` after the IDENT; consume the single space
                // that follows so canonical `IDENT = 'lit'` shape lands.
                let after: String = chars[col..].iter().collect();
                let before: String = chars[..col].iter().collect();
                let replacement = if let Some(rest) = after.strip_prefix(' ') {
                    format!(" = {}", rest)
                } else {
                    format!(" = {}", after)
                };
                *line = format!("{}{}", before, replacement);
            }
        }

        lines.join("\n")
    }
}

fn label_violations(tokens: &[Token]) -> Vec<(Token, Token)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if !is_label_statement(tokens, i) {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < tokens.len() && tokens[j].token_type != TokenType::SEMI {
            if tokens[j].token_type == TokenType::Identifier {
                if let Some(nxt) = tokens.get(j + 1) {
                    if nxt.token_type == TokenType::StringLiteral {
                        out.push((tokens[j].clone(), nxt.clone()));
                        j += 2;
                        continue;
                    }
                }
            }
            j += 1;
        }
        i = j + 1;
    }
    out
}

/// Statement keywords that can't continue a `label` statement. Seeing one
/// right after a label string means the `;` closing the label is missing.
const STATEMENT_KEYWORDS: &[TokenType] = &[
    TokenType::KwLabel,
    TokenType::KwAttrib,
    TokenType::KwFormat,
    TokenType::KwInformat,
    TokenType::KwLength,
    TokenType::KwIf,
    TokenType::KwElse,
    TokenType::KwDo,
    TokenType::KwEnd,
    TokenType::KwDrop,
    TokenType::KwKeep,
    TokenType::KwRun,
];

/// `(string, keyword)` pairs where a `label` statement's string is followed
/// by a statement keyword. A keyword followed by `=` is a variable that
/// happens to share the name (`label los = 'x' length = 'y'`), not a new
/// statement.
fn unterminated_labels(tokens: &[Token]) -> Vec<(Token, Token)> {
    let mut out = Vec::new();
    let mut in_label = false;
    for (j, t) in tokens.iter().enumerate() {
        match t.token_type {
            TokenType::SEMI => in_label = false,
            TokenType::KwLabel if is_label_statement(tokens, j) => in_label = true,
            TokenType::StringLiteral if in_label => {
                let (Some(kw), next) = (tokens.get(j + 1), tokens.get(j + 2)) else {
                    continue;
                };
                if STATEMENT_KEYWORDS.contains(&kw.token_type)
                    && next.map(|n| n.token_type) != Some(TokenType::ASSIGN)
                {
                    out.push((t.clone(), kw.clone()));
                }
            }
            _ => {}
        }
    }
    out
}

/// Insert the missing `;` straight after each label string that runs into
/// a statement keyword.
fn insert_label_semicolons(source: &str) -> String {
    let stream = TokenStream::tokenize(source);
    let mut out = source.to_string();
    let mut ends: Vec<usize> = unterminated_labels(&stream.default)
        .into_iter()
        .map(|(string, _)| string.end_byte as usize)
        .collect();
    ends.sort_unstable_by(|a, b| b.cmp(a));
    for end in ends {
        out.insert(end, ';');
    }
    out
}

/// A `label` keyword that opens a statement. `label=` options of `attrib`,
/// SAS/GRAPH `axis` and the like are not `label` statements.
fn is_label_statement(tokens: &[Token], i: usize) -> bool {
    tokens[i].token_type == TokenType::KwLabel
        && (i == 0 || tokens[i - 1].token_type == TokenType::SEMI)
}

/// The string's first line, cut to 40 characters, for a message.
fn shorten(text: &str) -> String {
    let first = text.lines().next().unwrap_or("");
    let chars: Vec<char> = first.chars().collect();
    if first.len() == text.len() && chars.len() <= 40 {
        text.to_string()
    } else {
        let head: String = chars.iter().take(37).collect();
        format!("{}...", head)
    }
}
