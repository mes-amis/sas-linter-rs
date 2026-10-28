use crate::config::{opt_bool, opt_i64};
use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Hook, Rule};
use crate::token::{in_ranges, literal_byte_ranges, TokenStream};

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
