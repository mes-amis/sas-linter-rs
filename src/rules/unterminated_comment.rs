use sas_lexer::{TokenChannel, TokenType};

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::{Token, TokenStream};

use super::{statement_not_valid, RuleMeta};

pub struct UnterminatedComment {
    autofix: bool,
}

const ID: &str = "unterminated_comment";
const DESCRIPTION: &str = "`*` / `**` comment with no `;` on its line runs on and \
                            swallows the code that follows, up to the next `;`.";

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
    fn severity(&self) -> Severity {
        // It silently disables code.
        Severity::Error
    }
    fn supports_autofix(&self) -> bool {
        true
    }
    fn autofix_enabled(&self) -> bool {
        self.autofix
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        swallows(&ctx.tokens.all, ctx.source)
            .into_iter()
            .map(|s| Finding {
                path: ctx.path.to_string(),
                line: s.comment.start_line,
                column: s.comment.start_column + 1,
                rule: ID,
                message: format!(
                    "`{}` comment has no `;` on its line, so it runs on and swallows \
                     the code on line {} — SAS never executes it. End the comment \
                     with `;`.",
                    if s.comment.text.starts_with("**") {
                        "**"
                    } else {
                        "*"
                    },
                    s.code_line
                ),
                severity: Severity::Error,
            })
            .collect()
    }

    fn autofix(&self, source: &str) -> String {
        let stream = TokenStream::tokenize(source);
        let mut edits: Vec<(usize, usize)> = swallows(&stream.all, source)
            .into_iter()
            .filter(|s| s.fixable)
            .map(|s| s.first_line_blank_tail)
            .collect();
        edits.sort_unstable_by(|a, b| b.cmp(a));
        let mut out = source.to_string();
        for (from, to) in edits {
            out.replace_range(from..to, ";");
        }
        out
    }
}

/// A `*` comment that ran past its first line and swallowed code.
pub(crate) struct Swallow<'a> {
    pub comment: &'a Token,
    /// 1-based line of the first swallowed line that is a SAS statement.
    pub code_line: u32,
    /// Byte range of the first line's trailing blanks — where the `;` goes.
    first_line_blank_tail: (usize, usize),
    /// The code sits right after the first line, so ending the comment
    /// there restores it without turning wrapped prose into code.
    fixable: bool,
}

/// `*` / `**` comments that span lines (SAS runs them to the next `;`)
/// where a swallowed line starts a SAS statement. A comment that runs to
/// end of file found no `;`, so nothing it swallowed was a complete
/// statement.
pub(crate) fn swallows<'a>(all_tokens: &'a [Token], source: &str) -> Vec<Swallow<'a>> {
    let mut out = Vec::new();
    for t in all_tokens {
        if t.channel != TokenChannel::COMMENT
            || t.token_type != TokenType::PredictedCommentStat
            || t.start_line >= t.end_line
        {
            continue;
        }
        let start = t.start_byte as usize;
        let text = &source[start..t.end_byte as usize];
        if !text.ends_with(';') {
            continue;
        }
        let Some(first_nl) = text.find('\n') else {
            continue;
        };
        let first = &text[..first_nl];
        let first = first.strip_suffix('\r').unwrap_or(first);
        let kept = first.trim_end_matches([' ', '\t']).len();

        let rest = &text[first_nl + 1..text.len() - 1];
        let mut seen_text = false;
        for (k, line) in rest.split('\n').enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            if is_statement(line) {
                out.push(Swallow {
                    comment: t,
                    code_line: t.start_line + 1 + k as u32,
                    first_line_blank_tail: (start + kept, start + first.len()),
                    fixable: !seen_text,
                });
                break;
            }
            seen_text = true;
        }
    }
    out
}

/// Whether a swallowed line, lexed on its own, starts a SAS statement —
/// the same test `statement_not_valid` applies to code.
fn is_statement(line: &str) -> bool {
    let stream = TokenStream::tokenize(line);
    let tokens: Vec<Token> = stream
        .default
        .into_iter()
        .filter(|t| t.token_type != TokenType::EOF)
        .collect();
    match tokens.first() {
        None => false,
        Some(first) if statement_not_valid::is_macro(first) => {
            first.token_type != TokenType::MacroComment
        }
        Some(_) => statement_not_valid::starts_validly(&tokens),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix(src: &str) -> String {
        UnterminatedComment { autofix: true }.autofix(src)
    }

    #[test]
    fn autofix_appends_semicolon_on_lf() {
        assert_eq!(
            fix("data x;\n** COMMENT **\nx = 1;\nrun;\n"),
            "data x;\n** COMMENT **;\nx = 1;\nrun;\n"
        );
    }

    #[test]
    fn autofix_inserts_semicolon_before_cr_on_crlf() {
        assert_eq!(
            fix("data x;\r\n** COMMENT **\r\nx = 1;\r\nrun;\r\n"),
            "data x;\r\n** COMMENT **;\r\nx = 1;\r\nrun;\r\n"
        );
    }

    #[test]
    fn autofix_strips_trailing_blanks_before_cr() {
        assert_eq!(
            fix("data x;\r\n** COMMENT ** \t\r\nx = 1;\r\nrun;\r\n"),
            "data x;\r\n** COMMENT **;\r\nx = 1;\r\nrun;\r\n"
        );
    }
}
