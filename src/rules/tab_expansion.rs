use std::ops::Range;

use sas_lexer::TokenType;

use crate::config::{opt_bool, opt_i64};
use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Hook, Rule};
use crate::token::TokenStream;

use super::RuleMeta;

pub struct TabExpansion {
    width: usize,
    autofix: bool,
}

const ID: &str = "tab_expansion";
const DESCRIPTION: &str = "Line contains a literal TAB character; will be expanded to spaces.";
const DEFAULT_WIDTH: usize = 8;

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: true,
        default_factory: || {
            Box::new(TabExpansion {
                width: DEFAULT_WIDTH,
                autofix: false,
            })
        },
        config_factory: |opts| {
            let width = opt_i64(opts, "width").unwrap_or(DEFAULT_WIDTH as i64);
            if width < 1 {
                anyhow::bail!("tab_expansion.width must be positive (got {})", width);
            }
            Ok(Box::new(TabExpansion {
                width: width as usize,
                autofix: opt_bool(opts, "autofix").unwrap_or(false),
            }))
        },
    }
}

impl Rule for TabExpansion {
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
    fn hook(&self) -> Hook {
        Hook::BeforeFormat
    }

    /// One finding per tab character outside string literals and datalines.
    /// Column is the 1-based character column of the tab in the original
    /// source (no expansion math applied).
    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        let literals = literal_byte_ranges(ctx.tokens);
        let mut findings = Vec::new();
        let mut line: u32 = 1;
        let mut col: u32 = 1;
        for (i, ch) in ctx.source.char_indices() {
            if ch == '\n' {
                line += 1;
                col = 1;
                continue;
            }
            if ch == '\t' && !in_ranges(&literals, i) {
                let msg = if self.autofix {
                    format!("tab character (expanded to {}-space tab stop)", self.width)
                } else {
                    "tab character".to_string()
                };
                findings.push(Finding {
                    path: ctx.path.to_string(),
                    line,
                    column: col,
                    rule: ID,
                    message: msg,
                    severity: Severity::Warning,
                });
            }
            col += 1;
        }
        findings
    }

    /// Each tab expands to `width - (column % width)` spaces — standard
    /// `expand(1)` semantics so column-aligned content stays aligned. Tabs
    /// inside string literals and datalines are data, not layout, and are
    /// kept (they still occupy one column for the tab stops after them).
    fn autofix(&self, source: &str) -> String {
        let literals = literal_byte_ranges(&TokenStream::tokenize(source));
        let mut out = String::with_capacity(source.len());
        let mut col = 0usize;
        for (i, ch) in source.char_indices() {
            if ch == '\t' && !in_ranges(&literals, i) {
                let pad = self.width - (col % self.width);
                out.extend(std::iter::repeat_n(' ', pad));
                col += pad;
                continue;
            }
            out.push(ch);
            col = if ch == '\n' { 0 } else { col + 1 };
        }
        out
    }
}

/// Byte ranges of the tokenized source whose contents are data rather than
/// layout: string literals (plain, typed like `'09'x`, and macro-resolving
/// `"…&mv…"` expressions, any of which may span lines) and datalines card
/// images. Whitespace and byte-rewriting rules must leave these bytes alone.
/// Sorted and non-overlapping; an unterminated string runs to EOF.
pub(crate) fn literal_byte_ranges(tokens: &TokenStream) -> Vec<Range<usize>> {
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
pub(crate) fn in_ranges(ranges: &[Range<usize>], pos: usize) -> bool {
    let idx = ranges.partition_point(|r| r.end <= pos);
    ranges.get(idx).is_some_and(|r| r.start <= pos)
}
