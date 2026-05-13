use std::collections::HashSet;

use once_cell::sync::Lazy;
use regex::Regex;
use sas_lexer::{lex_program, TokenChannel, TokenType};

/// Flattened token record matching the shape the Ruby gem exposed via FFI.
///
/// Fields:
/// * `token_type`, `channel` — from sas-lexer enums (`TokenType`, `TokenChannel`).
/// * `text` — slice of the original source covered by this token. Empty for
///   zero-width tokens (e.g. synthetic separators).
/// * `start_line` / `start_column` — 1-based line, 0-based column (matching
///   the lexer; rule reports add +1 to the column before emitting).
/// * `end_line` / `end_column` — exclusive end position; `end_column` equals
///   the column AFTER the last character of the token, on `end_line`.
/// * `start_byte` / `end_byte` — byte offsets into the source string. Used by
///   autofix passes that splice the source by byte range.
#[derive(Debug, Clone)]
pub struct Token {
    pub token_type: TokenType,
    pub channel: TokenChannel,
    pub text: String,
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub start_byte: u32,
    pub end_byte: u32,
}

/// Both channels in one bundle — `default` excludes whitespace + comments
/// (the channel most rules walk); `all` retains every token (needed by
/// comment-aware rules like `commented_out_guard` and `unterminated_comment`).
#[derive(Debug, Clone)]
pub struct TokenStream {
    pub default: Vec<Token>,
    pub all: Vec<Token>,
}

impl TokenStream {
    pub fn tokenize(source: &str) -> Self {
        let result = match lex_program(&source) {
            Ok(r) => r,
            Err(_) => {
                return TokenStream {
                    default: Vec::new(),
                    all: Vec::new(),
                };
            }
        };

        let buffer = &result.buffer;
        let mut all: Vec<Token> = Vec::with_capacity(buffer.token_count() as usize);

        for idx in buffer.iter_tokens() {
            let token_type = match buffer.get_token_type(idx) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let channel = match buffer.get_token_channel(idx) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let start_byte = buffer
                .get_token_start_byte_offset(idx)
                .map(u32::from)
                .unwrap_or(0);
            let end_byte = buffer
                .get_token_end_byte_offset(idx)
                .map(u32::from)
                .unwrap_or(start_byte);
            let start_line = buffer.get_token_start_line(idx).unwrap_or(1);
            let start_column = buffer.get_token_start_column(idx).unwrap_or(0);
            let end_line = buffer.get_token_end_line(idx).unwrap_or(start_line);
            let end_column = buffer.get_token_end_column(idx).unwrap_or(start_column);
            let text = source
                .get(start_byte as usize..end_byte as usize)
                .unwrap_or("")
                .to_string();

            all.push(Token {
                token_type,
                channel,
                text,
                start_line,
                start_column,
                end_line,
                end_column,
                start_byte,
                end_byte,
            });
        }

        // Lines like `**  text **;` are a project-level "visual comment"
        // idiom. SAS itself terminates `*…;` at the first `;`, so prose with
        // embedded `;` (e.g. revision dates `01/01/00;`) ends the comment
        // early and the remainder of the line lexes as code. Treat any line
        // that opens with `**` and closes with `**;` as a comment for
        // analysis purposes, regardless of internal semicolons.
        let visual_lines = visual_comment_lines(source);

        let default: Vec<Token> = all
            .iter()
            .filter(|t| !matches!(t.channel, TokenChannel::HIDDEN | TokenChannel::COMMENT))
            .filter(|t| !visual_lines.contains(&t.start_line))
            .cloned()
            .collect();

        TokenStream { default, all }
    }
}

static VISUAL_COMMENT_LINE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*\*\*.*\*\*;\s*$").unwrap());

fn visual_comment_lines(source: &str) -> HashSet<u32> {
    let mut set = HashSet::new();
    for (idx, line) in source.lines().enumerate() {
        if VISUAL_COMMENT_LINE.is_match(line) {
            set.insert((idx + 1) as u32);
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_visual_comment_lines() {
        let src = "data foo;\n\
                   **  REVISION DATES:   01/01/00; 02/02/01 **;\n\
                   **  ALGORITHM: splits sample by A, then B **;\n\
                   x = 1;\n\
                   **A**; **B**;\n";
        let lines = visual_comment_lines(src);
        assert!(lines.contains(&2));
        assert!(lines.contains(&3));
        assert!(lines.contains(&5));
        assert!(!lines.contains(&1));
        assert!(!lines.contains(&4));
    }

    #[test]
    fn ignores_partial_visual_comment_lines() {
        let src = "** comment **; data foo;\n\
                   **; trailing\n\
                   * single-star comment;\n";
        let lines = visual_comment_lines(src);
        assert!(lines.is_empty());
    }
}
