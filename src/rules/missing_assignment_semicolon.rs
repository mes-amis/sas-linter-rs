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
                            `**` comment marker was lexed as exponentiation.";

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
        lines.join("\n")
    }
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
