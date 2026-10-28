use std::ops::Range;

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
    /// Tokenize `source`, or an empty stream if the lexer refuses it (only a
    /// source over 4 GiB). Rules re-tokenize their own output with this;
    /// file entry points check with [`TokenStream::try_tokenize`] first so a
    /// refused file is an error, not a clean result.
    pub fn tokenize(source: &str) -> Self {
        Self::try_tokenize(source).unwrap_or(TokenStream {
            default: Vec::new(),
            all: Vec::new(),
        })
    }

    pub fn try_tokenize(source: &str) -> Result<Self, sas_lexer::error::ErrorKind> {
        let result = lex_program(&source)?;

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

        let default: Vec<Token> = all
            .iter()
            .filter(|t| !matches!(t.channel, TokenChannel::HIDDEN | TokenChannel::COMMENT))
            .cloned()
            .collect();

        Ok(TokenStream { default, all })
    }
}

/// Byte ranges of the tokenized source whose contents are data rather than
/// layout: string literals (plain, typed like `'09'x`, and macro-resolving
/// `"…&mv…"` expressions, any of which may span lines) and datalines card
/// images. Whitespace and byte-rewriting rules must leave these bytes alone.
/// Sorted and non-overlapping; an unterminated string runs to EOF.
pub fn literal_byte_ranges(tokens: &TokenStream) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut expr_start: Option<usize> = None;
    let mut depth = 0usize;
    for t in &tokens.all {
        let (start, end) = (t.start_byte as usize, t.end_byte as usize);
        match t.token_type {
            TokenType::StringLiteral
            | TokenType::BitTestingLiteral
            | TokenType::DateLiteral
            | TokenType::DateTimeLiteral
            | TokenType::NameLiteral
            | TokenType::TimeLiteral
            | TokenType::HexStringLiteral
                if depth == 0 =>
            {
                ranges.push(start..end)
            }
            TokenType::StringExprStart => {
                if depth == 0 {
                    expr_start = Some(start);
                }
                depth += 1;
            }
            TokenType::StringExprEnd
            | TokenType::BitTestingLiteralExprEnd
            | TokenType::DateLiteralExprEnd
            | TokenType::DateTimeLiteralExprEnd
            | TokenType::NameLiteralExprEnd
            | TokenType::TimeLiteralExprEnd
            | TokenType::HexStringLiteralExprEnd => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if let Some(s) = expr_start.take() {
                        ranges.push(s..end);
                    }
                }
            }
            // Card images start on the line after `datalines;` — SAS
            // ignores the rest of the statement's own line, so it stays
            // layout.
            TokenType::DatalinesData => {
                let first_nl = t.text.find('\n').map_or(t.text.len(), |n| n + 1);
                ranges.push(start + first_nl..end);
            }
            _ => {}
        }
    }
    if let (Some(s), Some(last)) = (expr_start, tokens.all.last()) {
        ranges.push(s..last.end_byte as usize);
    }
    ranges
}

/// Whether byte offset `pos` falls inside one of the sorted `ranges`.
pub fn in_ranges(ranges: &[Range<usize>], pos: usize) -> bool {
    let idx = ranges.partition_point(|r| r.end <= pos);
    ranges.get(idx).is_some_and(|r| r.start <= pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_after_a_semicolon_in_a_double_star_line_is_code() {
        // SAS ends the comment at the first `;`; the rest of the line runs.
        let src = "data foo;\n**  REVISION DATES:   01/01/00; 02/02/01 **;\nx = 1;\n";
        let stream = TokenStream::tokenize(src);
        assert!(
            stream
                .default
                .iter()
                .any(|t| t.start_line == 2 && t.text == "02"),
            "{:?}",
            stream.default
        );
    }
}
