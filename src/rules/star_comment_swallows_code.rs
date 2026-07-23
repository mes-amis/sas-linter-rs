use once_cell::sync::Lazy;
use regex::Regex;
use sas_lexer::{TokenChannel, TokenType};

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};

use super::RuleMeta;

pub struct StarCommentSwallowsCode;

const ID: &str = "star_comment_swallows_code";
const DESCRIPTION: &str = "Trailing `*` comment not terminated on its own line \
                            (commonly ending in `:`) — silently consumes the \
                            following statement up to the next `;`.";

/// The following swallowed line is one of these — a comment or a block/step
/// close — rather than an executable statement. In that case the comment
/// almost certainly isn't disabling real work, so we stay quiet (matches the
/// issue's "not another comment / `end;` / block close" carve-out).
static NON_STATEMENT_FOLLOWER: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^\s*(\*|/\*|end\b|run\b|quit\b|%mend\b)").unwrap());

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: false,
        default_factory: || Box::new(StarCommentSwallowsCode),
        config_factory: |_| Ok(Box::new(StarCommentSwallowsCode)),
    }
}

impl Rule for StarCommentSwallowsCode {
    fn id(&self) -> &'static str {
        ID
    }
    fn description(&self) -> &'static str {
        DESCRIPTION
    }
    fn severity(&self) -> Severity {
        // High severity: it silently disables code.
        Severity::Error
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        let lines: Vec<&str> = ctx.source.split('\n').collect();
        let mut findings = Vec::new();

        for t in &ctx.tokens.all {
            if t.channel != TokenChannel::COMMENT {
                continue;
            }
            if t.token_type != TokenType::PredictedCommentStat {
                continue;
            }
            // Single `*` only. `**`-style visual boxes are owned by the
            // `unterminated_comment` rule.
            if !t.text.starts_with('*') || t.text.starts_with("**") {
                continue;
            }
            // The comment didn't terminate on its own line: it ran past the
            // end of the opening line to find its `;` — i.e. no `;` before
            // EOL, so it swallowed whatever followed.
            if t.start_line >= t.end_line {
                continue;
            }
            // Trailing comment: there is code before the `*` on the opening
            // line. A `*` in statement position is only a comment after a
            // preceding `;`, so this is the "opens after a `;`" form. A
            // full-line `*` comment (no code before it) is excluded — those
            // are the lower-signal, often-intentional wrapping comments.
            let opens_after_code = ctx.tokens.default.iter().any(|d| {
                d.channel == TokenChannel::DEFAULT
                    && d.start_line == t.start_line
                    && d.start_column < t.start_column
            });
            if !opens_after_code {
                continue;
            }
            // The first non-blank swallowed line should look like an
            // executable statement, not another comment / block close.
            if !first_swallowed_line_is_statement(&lines, t.start_line, t.end_line) {
                continue;
            }

            findings.push(Finding {
                path: ctx.path.to_string(),
                line: t.start_line,
                column: t.start_column + 1,
                rule: ID,
                message: format!(
                    "`*` comment starting at line {} is not terminated on its own line; \
                     it will consume the following statement(s) until the next `;`. \
                     Did you mean to end the comment with `;`?",
                    t.start_line
                ),
                severity: Severity::Error,
            });
        }

        findings
    }
}

/// True when the first non-blank source line *after* `start_line` (up to and
/// including `end_line`) reads like a statement rather than a comment or a
/// block/step close. Lines are 1-based; `lines` is 0-indexed.
fn first_swallowed_line_is_statement(lines: &[&str], start_line: u32, end_line: u32) -> bool {
    for ln in (start_line + 1)..=end_line {
        let Some(text) = lines.get((ln - 1) as usize) else {
            break;
        };
        if text.trim().is_empty() {
            continue;
        }
        return !NON_STATEMENT_FOLLOWER.is_match(text);
    }
    false
}
