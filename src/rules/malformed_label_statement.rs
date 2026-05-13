use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::{Token, TokenStream};

use super::RuleMeta;

pub struct MalformedLabelStatement {
    autofix: bool,
}

const ID: &str = "malformed_label_statement";
const DESCRIPTION: &str = "`label` statement missing `=` between variable and string literal.";

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
        findings
    }

    fn autofix(&self, source: &str) -> String {
        if source.is_empty() {
            return source.to_string();
        }
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
        if tokens[i].token_type != TokenType::KwLabel {
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

fn shorten(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= 40 {
        text.to_string()
    } else {
        let head: String = chars[..37].iter().collect();
        format!("{}...", head)
    }
}
