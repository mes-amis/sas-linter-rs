use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::Token;

use super::RuleMeta;

pub struct UnterminatedBlockComment;

const ID: &str = "unterminated_block_comment";
const DESCRIPTION: &str = "`/*` comment never closed with `*/` runs to end of file and \
                            swallows whatever is submitted after the program.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: false,
        default_factory: || Box::new(UnterminatedBlockComment),
        config_factory: |_opts| Ok(Box::new(UnterminatedBlockComment)),
    }
}

impl Rule for UnterminatedBlockComment {
    fn id(&self) -> &'static str {
        ID
    }
    fn description(&self) -> &'static str {
        DESCRIPTION
    }
    fn severity(&self) -> Severity {
        // It silently disables whatever follows the file.
        Severity::Error
    }

    /// Report-only by design: appending `*/` at end of file would keep
    /// whatever the comment swallowed commented out, and where the `*/`
    /// belongs is a judgment call.
    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        ctx.tokens
            .all
            .iter()
            .filter(|t| is_unterminated(t))
            .map(|t| Finding {
                path: ctx.path.to_string(),
                line: t.start_line,
                column: t.start_column + 1,
                rule: ID,
                message: "`/*` comment is never closed with `*/`, so it runs to end of \
                          file and swallows anything submitted after this program — \
                          a following `run;` never executes. Close it with `*/`, or use \
                          a `*` statement comment ending in `;`."
                    .to_string(),
                severity: Severity::Error,
            })
            .collect()
    }
}

/// The lexer emits an unclosed `/*` as a `CStyleComment` running to EOF
/// (alongside its `UnterminatedComment` error), so the token text is the
/// whole tail of the file. A closed one ends in `*/` past the opening `/*`
/// — `/*/` does not count.
fn is_unterminated(t: &Token) -> bool {
    t.token_type == TokenType::CStyleComment && !(t.text.len() >= 4 && t.text.ends_with("*/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::TokenStream;

    fn lines(src: &str) -> Vec<u32> {
        let tokens = TokenStream::tokenize(src);
        let ctx = CheckContext {
            tokens: &tokens,
            source: src,
            path: "t.sas",
        };
        UnterminatedBlockComment
            .check(&ctx)
            .into_iter()
            .map(|f| f.line)
            .collect()
    }

    #[test]
    fn flags_banner_written_as_open_comment() {
        assert_eq!(lines("data out;\n  set in;\nrun;\n\n/*END OF CODE;\n"), [5]);
    }

    #[test]
    fn flags_commented_out_block_missing_close() {
        assert_eq!(
            lines("data out;\nrun;\n/*\n  compute score = a + b.\n  execute.\n"),
            [3]
        );
    }

    #[test]
    fn flags_close_typed_as_open() {
        assert_eq!(lines("/* old code\nx = 1;\n/*\nrun;\n"), [1]);
    }

    #[test]
    fn closed_comments_are_silent() {
        assert!(lines("/**/\n/* a */ data x; /* multi\nline */ run;\n/***/\n").is_empty());
    }

    #[test]
    fn slash_star_slash_is_not_closed() {
        assert_eq!(lines("data x; run;\n/*/\n"), [2]);
    }

    #[test]
    fn open_comment_inside_string_is_silent() {
        assert!(lines("data x; y = '/*'; run;\n").is_empty());
    }
}
