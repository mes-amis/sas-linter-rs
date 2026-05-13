use once_cell::sync::Lazy;
use regex::Regex;

use crate::config::opt_bool;
use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};

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

// `(ws)(line terminator or EOF)` — anchor the terminator so the autofix
// can preserve it (`$2` in Ruby).
static TRAILING_WS: Lazy<Regex> = Lazy::new(|| Regex::new(r"([ \t]+)(\r?\n|\z)").unwrap());

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
        let mut findings = Vec::new();
        for (idx, raw_line) in ctx.source.split_inclusive('\n').enumerate() {
            let line = raw_line.trim_end_matches('\n').trim_end_matches('\r');
            let trimmed = line.trim_end_matches([' ', '\t']);
            if trimmed.len() == line.len() {
                continue;
            }
            let col = trimmed.chars().count() + 1; // 1-based
            findings.push(Finding {
                path: ctx.path.to_string(),
                line: (idx + 1) as u32,
                column: col as u32,
                rule: ID,
                message: if self.autofix {
                    "trailing whitespace (autofixed)".into()
                } else {
                    "trailing whitespace".into()
                },
                severity: Severity::Warning,
            });
        }
        findings
    }

    fn autofix(&self, source: &str) -> String {
        TRAILING_WS.replace_all(source, "$2").into_owned()
    }
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
