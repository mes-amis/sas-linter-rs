use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::{Token, TokenStream};

use super::RuleMeta;

pub struct UnescapedQuoteInString {
    autofix: bool,
}

const ID: &str = "unescaped_quote_in_string";
const DESCRIPTION: &str = "Quote inside a string that matches its delimiter \
                            (`'Children's …'`) ends the string early; the rest \
                            runs on as code and a new string.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: true,
        default_factory: || Box::new(UnescapedQuoteInString { autofix: false }),
        config_factory: |opts| {
            Ok(Box::new(UnescapedQuoteInString {
                autofix: crate::config::opt_bool(opts, "autofix").unwrap_or(false),
            }))
        },
    }
}

impl Rule for UnescapedQuoteInString {
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
        early_ends(&ctx.tokens.default)
            .into_iter()
            .map(|(string, word)| {
                let quote = closing_quote(string);
                Finding {
                    path: ctx.path.to_string(),
                    line: string.end_line,
                    column: string.end_column,
                    rule: ID,
                    message: format!(
                        "`{quote}` before `{}` ends the string early — SAS reads the \
                         rest as code and a new string. Double it (`{quote}{quote}`) \
                         to keep it in the string.",
                        word.text
                    ),
                    severity: Severity::Error,
                }
            })
            .collect()
    }

    /// Double the first early-ending quote, re-lex, and repeat: doubling one
    /// changes where every later string in the file starts and ends.
    fn autofix(&self, source: &str) -> String {
        let mut out = source.to_string();
        for _ in 0..100 {
            let stream = TokenStream::tokenize(&out);
            let Some((string, _)) = early_ends(&stream.default).into_iter().next() else {
                break;
            };
            let end = string.end_byte as usize;
            out.insert(end, closing_quote(string));
        }
        out
    }
}

fn closing_quote(string: &Token) -> char {
    string.text.chars().last().unwrap_or('\'')
}

/// One-line string literals followed with no gap by an identifier. Typed
/// literals (`'…'d`, `'…'n`, `'…'x`, …) lex as their own token types and
/// keywords (`'a'then`) aren't identifiers, so a word glued to the closing
/// quote means the quote was an apostrophe in the text. A string that ran
/// on from an earlier line went wrong where it opened, not here.
fn early_ends(tokens: &[Token]) -> Vec<(&Token, &Token)> {
    tokens
        .windows(2)
        .filter(|w| {
            w[0].token_type == TokenType::StringLiteral
                && w[0].start_line == w[0].end_line
                && w[1].token_type == TokenType::Identifier
                && w[1].start_byte == w[0].end_byte
        })
        .map(|w| (&w[0], &w[1]))
        .collect()
}
