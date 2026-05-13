use crate::config::{opt_bool, opt_i64};
use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};

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

    /// One finding per tab character. Column is the 1-based character
    /// column of the tab in the original source (no expansion math applied).
    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        let mut findings = Vec::new();
        for (idx, raw_line) in ctx.source.split_inclusive('\n').enumerate() {
            let body = raw_line.trim_end_matches('\n').trim_end_matches('\r');
            if !body.contains('\t') {
                continue;
            }
            for (col, ch) in body.chars().enumerate() {
                if ch == '\t' {
                    let msg = if self.autofix {
                        format!("tab character (expanded to {}-space tab stop)", self.width)
                    } else {
                        "tab character".to_string()
                    };
                    findings.push(Finding {
                        path: ctx.path.to_string(),
                        line: (idx + 1) as u32,
                        column: (col + 1) as u32,
                        rule: ID,
                        message: msg,
                        severity: Severity::Warning,
                    });
                }
            }
        }
        findings
    }

    /// Each tab expands to `width - (out.len() % width)` spaces — standard
    /// `expand(1)` semantics so column-aligned content stays aligned.
    fn autofix(&self, source: &str) -> String {
        let mut out = String::with_capacity(source.len());
        for raw_line in source.split_inclusive('\n') {
            let mut term = "";
            let body = if let Some(rest) = raw_line.strip_suffix("\r\n") {
                term = "\r\n";
                rest
            } else if let Some(rest) = raw_line.strip_suffix('\n') {
                term = "\n";
                rest
            } else {
                raw_line
            };

            let mut line_out = String::with_capacity(body.len());
            for ch in body.chars() {
                if ch == '\t' {
                    let pad = self.width - (line_out.chars().count() % self.width);
                    for _ in 0..pad {
                        line_out.push(' ');
                    }
                } else {
                    line_out.push(ch);
                }
            }
            out.push_str(&line_out);
            out.push_str(term);
        }
        out
    }
}
