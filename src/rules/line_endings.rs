use crate::config::opt_bool;
use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};

use super::RuleMeta;

pub struct LineEndings {
    autofix: bool,
}

const ID: &str = "line_endings";
const DESCRIPTION: &str = "Source has non-standard line endings (double-CR or lone CR).";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: true,
        default_factory: || Box::new(LineEndings { autofix: false }),
        config_factory: |opts| {
            Ok(Box::new(LineEndings {
                autofix: opt_bool(opts, "autofix").unwrap_or(false),
            }))
        },
    }
}

impl Rule for LineEndings {
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

    /// Two patterns matter:
    ///
    ///   1. `\r\r\n` — Word/Outlook injects double CR. Downstream diffs
    ///      treat every line as having a trailing literal CR.
    ///   2. Lone `\r` — old-Mac line endings. SAS Viya treats the entire
    ///      file as one logical line, breaking saspy submission.
    ///
    /// Walk bytes manually so column math stays right when the source
    /// contains multi-byte UTF-8 sequences before a problem CR.
    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        let bytes = ctx.source.as_bytes();
        let mut findings = Vec::new();
        let mut line: u32 = 1;
        let mut col: u32 = 1;
        let n = bytes.len();
        let mut i = 0;

        while i < n {
            let b = bytes[i];
            if b == 0x0D && i + 2 < n && bytes[i + 1] == 0x0D && bytes[i + 2] == 0x0A {
                findings.push(self.finding(ctx.path, line, col, "double CR before LF (\\r\\r\\n)"));
                line += 1;
                col = 1;
                i += 3;
            } else if b == 0x0D && i + 1 < n && bytes[i + 1] == 0x0A {
                line += 1;
                col = 1;
                i += 2;
            } else if b == 0x0D {
                findings.push(self.finding(ctx.path, line, col, "lone CR (\\r)"));
                line += 1;
                col = 1;
                i += 1;
            } else if b == 0x0A {
                line += 1;
                col = 1;
                i += 1;
            } else {
                col += 1;
                i += 1;
            }
        }

        findings
    }

    /// Collapse `\r\r\n` to `\r\n`, then map every remaining lone `\r` to
    /// the file's dominant terminator (`\r\n` if any survived, else `\n`).
    fn autofix(&self, source: &str) -> String {
        let step1 = source.replace("\r\r\n", "\r\n");
        let dominant_crlf = step1.contains("\r\n");
        let replacement = if dominant_crlf { "\r\n" } else { "\n" };

        let mut out = String::with_capacity(step1.len());
        let mut chars = step1.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\r' {
                if chars.peek() == Some(&'\n') {
                    out.push('\r');
                    out.push('\n');
                    chars.next();
                } else {
                    out.push_str(replacement);
                }
            } else {
                out.push(c);
            }
        }
        out
    }
}

impl LineEndings {
    fn finding(&self, path: &str, line: u32, column: u32, msg: &str) -> Finding {
        let suffix = if self.autofix { " (autofixed)" } else { "" };
        Finding {
            path: path.to_string(),
            line,
            column,
            rule: ID,
            message: format!("{}{}", msg, suffix),
            severity: Severity::Warning,
        }
    }
}
