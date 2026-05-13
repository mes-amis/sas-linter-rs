use sas_lexer::{TokenChannel, TokenType};

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::TokenStream;

use super::RuleMeta;

pub struct UnterminatedComment {
    autofix: bool,
}

const ID: &str = "unterminated_comment";
const DESCRIPTION: &str = "`**` comment missing its terminating `;` — consumes following code.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: true,
        default_factory: || Box::new(UnterminatedComment { autofix: false }),
        config_factory: |opts| {
            Ok(Box::new(UnterminatedComment {
                autofix: crate::config::opt_bool(opts, "autofix").unwrap_or(false),
            }))
        },
    }
}

impl Rule for UnterminatedComment {
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
        let lines: Vec<&str> = split_keep_empty(ctx.source);
        bad_line_indices(&ctx.tokens.all, &lines)
            .into_iter()
            .map(|i| {
                let line = lines[i];
                let indent = line.len() - line.trim_start().len();
                Finding {
                    path: ctx.path.to_string(),
                    line: (i + 1) as u32,
                    column: (indent + 1) as u32,
                    rule: ID,
                    message:
                        "`**` comment missing `;` — consumes the next line of code as comment text."
                            .into(),
                    severity: Severity::Warning,
                }
            })
            .collect()
    }

    fn autofix(&self, source: &str) -> String {
        if source.is_empty() {
            return source.to_string();
        }
        let stream = TokenStream::tokenize(source);
        let lines: Vec<&str> = split_keep_empty(source);
        let bad = bad_line_indices(&stream.all, &lines);
        if bad.is_empty() {
            return source.to_string();
        }
        let mut owned: Vec<String> = lines.iter().map(|s| (*s).to_string()).collect();
        for i in bad {
            let trimmed = owned[i].trim_end_matches([' ', '\t']).to_string();
            owned[i] = format!("{};", trimmed);
        }
        owned.join("\n")
    }
}

/// 0-indexed source lines that hold a `** ... **` comment whose missing
/// `;` made the lexer extend it into the next line. Mirrors Ruby:
///   - PredictedCommentStat on COMMENT channel
///   - start_line < end_line
///   - first source line rstripped ends with `**`
fn bad_line_indices(all_tokens: &[crate::token::Token], lines: &[&str]) -> Vec<usize> {
    let mut bad = Vec::new();
    for t in all_tokens {
        if t.channel != TokenChannel::COMMENT {
            continue;
        }
        if t.token_type != TokenType::PredictedCommentStat {
            continue;
        }
        if t.start_line >= t.end_line {
            continue;
        }
        let idx = (t.start_line as usize).saturating_sub(1);
        let Some(first) = lines.get(idx) else {
            continue;
        };
        if !first.trim_end().ends_with("**") {
            continue;
        }
        bad.push(idx);
    }
    bad
}

/// Match Ruby's `source.split("\n", -1)`: a trailing newline produces a
/// final empty string. `str::split('\n')` already does this.
fn split_keep_empty(s: &str) -> Vec<&str> {
    s.split('\n').collect()
}
