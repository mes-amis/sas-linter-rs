use std::ops::Range;

use crate::config::opt_bool;
use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::TokenStream;

use super::tab_expansion::{in_ranges, literal_byte_ranges};
use super::RuleMeta;

pub struct TrailingWhitespace {
    autofix: bool,
}

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: true,
        default_factory: || Box::new(TrailingWhitespace { autofix: false }),
        config_factory: |opts| {
            let r: Box<dyn Rule> = Box::new(TrailingWhitespace {
                autofix: opt_bool(opts, "autofix").unwrap_or(false),
            });
            Ok(r)
        },
    }
}

const ID: &str = "trailing_whitespace";
const DESCRIPTION: &str = "Line has trailing whitespace before the newline.";

impl Rule for TrailingWhitespace {
    fn id(&self) -> &'static str {
        ID
    }
    fn description(&self) -> &'static str {
        DESCRIPTION
    }
    fn severity(&self) -> Severity {
        Severity::Warning
    }
    fn supports_autofix(&self) -> bool {
        true
    }
    fn autofix_enabled(&self) -> bool {
        self.autofix
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        trailing_runs(ctx.source, ctx.tokens)
            .into_iter()
            .map(|run| Finding {
                path: ctx.path.to_string(),
                line: run.line,
                column: run.column,
                rule: ID,
                message: if self.autofix {
                    "trailing whitespace (autofixed)".into()
                } else {
                    "trailing whitespace".into()
                },
                severity: Severity::Warning,
            })
            .collect()
    }

    /// Drop each run reported by `check`; line terminators are untouched.
    fn autofix(&self, source: &str) -> String {
        let mut out = String::with_capacity(source.len());
        let mut copied = 0;
        for run in trailing_runs(source, &TokenStream::tokenize(source)) {
            out.push_str(&source[copied..run.bytes.start]);
            copied = run.bytes.end;
        }
        out.push_str(&source[copied..]);
        out
    }
}

/// A run of blanks between the last visible character and the line
/// terminator (or EOF). `column` is the 1-based character column of its
/// first blank.
struct TrailingRun {
    line: u32,
    column: u32,
    bytes: Range<usize>,
}

/// Trailing blank runs outside string literals and datalines: inside a
/// multi-line string the blanks are part of the value, and in datalines
/// they can be part of a fixed-width card image.
fn trailing_runs(source: &str, tokens: &TokenStream) -> Vec<TrailingRun> {
    let literals = literal_byte_ranges(tokens);
    let mut runs = Vec::new();
    let mut offset = 0;
    for (idx, raw_line) in source.split_inclusive('\n').enumerate() {
        let line = raw_line.trim_end_matches('\n').trim_end_matches('\r');
        let trimmed = line.trim_end_matches([' ', '\t']);
        let start = offset + trimmed.len();
        offset += raw_line.len();
        if trimmed.len() == line.len() || in_ranges(&literals, start) {
            continue;
        }
        runs.push(TrailingRun {
            line: (idx + 1) as u32,
            column: (trimmed.chars().count() + 1) as u32,
            bytes: start..start + (line.len() - trimmed.len()),
        });
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::TokenStream;

    #[test]
    fn flags_trailing_spaces_and_tabs() {
        let src = "abc   \n\tdef\t\nok\n";
        let stream = TokenStream::tokenize(src);
        let rule = TrailingWhitespace { autofix: false };
        let ctx = CheckContext {
            tokens: &stream,
            source: src,
            path: "(test)",
        };
        let findings = rule.check(&ctx);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].line, 1);
        assert_eq!(findings[1].line, 2);
    }

    #[test]
    fn autofix_strips_ws_and_preserves_terminator() {
        let rule = TrailingWhitespace { autofix: true };
        let src = "a   \nb\t\nok\r\nz \r\n";
        let fixed = rule.autofix(src);
        assert_eq!(fixed, "a\nb\nok\r\nz\r\n");
    }
}
