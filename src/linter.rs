use anyhow::Result;
use std::path::Path;

use crate::config::Config;
use crate::encoding;
use crate::finding::Finding;
use crate::rule::{CheckContext, Hook, Rule};
use crate::rules;
use crate::token::TokenStream;

pub struct Linter {
    pub rules: Vec<Box<dyn Rule>>,
}

impl Linter {
    /// All built-in rules at their defaults — used when no config is given
    /// or for `--list-rules`.
    pub fn with_default_rules() -> Self {
        Linter {
            rules: rules::default_rules(),
        }
    }

    /// Subset of rules by id, all with default options.
    pub fn from_ids(ids: &[String]) -> Result<Self> {
        let rules = rules::rules_by_ids(ids)?;
        Ok(Linter { rules })
    }

    /// Build from a parsed `Config`. Rules listed `enabled: false` are
    /// skipped; rules omitted entirely default to enabled with default
    /// options (matches the Ruby gem's behavior so new rules don't
    /// silently regress for existing users).
    pub fn from_config(config: &Config) -> Result<Self> {
        let rules = rules::rules_from_config(config)?;
        Ok(Linter { rules })
    }

    /// Lint a SAS source string. `path` is used for finding location output.
    pub fn lint(&self, source: &str, path: &str) -> Vec<Finding> {
        self.lint_with_fixes(source, path).0
    }

    /// Returns `(findings, modified_source)`. When no rule has autofix
    /// enabled, `modified_source` equals the input.
    pub fn lint_with_fixes(&self, source: &str, path: &str) -> (Vec<Finding>, String) {
        let tokens = TokenStream::tokenize(source);
        let ctx = CheckContext {
            tokens: &tokens,
            source,
            path,
        };
        let mut findings = Vec::new();
        for rule in &self.rules {
            findings.extend(rule.check(&ctx));
        }
        let findings = apply_report_precedence(findings);

        let mut modified = source.to_string();
        for rule in self.fixers(|_| true) {
            modified = rule.autofix(&modified);
        }

        (findings, modified)
    }

    /// Autofix-enabled rules accepted by `filter`, in `AUTOFIX_ORDER`.
    fn fixers(&self, filter: impl Fn(&dyn Rule) -> bool) -> Vec<&dyn Rule> {
        let mut fixers: Vec<&dyn Rule> = self
            .rules
            .iter()
            .map(|r| r.as_ref())
            .filter(|r| r.autofix_enabled() && r.supports_autofix() && filter(*r))
            .collect();
        fixers.sort_by_key(|r| {
            rules::AUTOFIX_ORDER
                .iter()
                .position(|id| *id == r.id())
                .unwrap_or(usize::MAX)
        });
        fixers
    }

    /// Lint a file by path. Sources are commonly Win-1252 / Latin-1 rather
    /// than UTF-8 — transcoded by `encoding::read_source` before tokenizing.
    /// Writes the modified source back if (and only if) any autofix-enabled
    /// rule produced a byte-different output.
    pub fn lint_file(&self, path: &Path) -> Result<Vec<Finding>> {
        let original = encoding::read_source(path)?;
        let (findings, modified) = self.lint_with_fixes(&original, &path.display().to_string());
        if modified.as_bytes() != original.as_bytes() {
            std::fs::write(path, modified.as_bytes())?;
        }
        Ok(findings)
    }

    /// Format a source string: `before_format` autofixes (by default
    /// encoding, line endings, tabs — see `Rule::hook`), then the
    /// formatter, then the `after_format` autofixes. Normalizing first
    /// means the formatter's column math sees the final bytes, so one
    /// save settles.
    pub fn format_source(&self, source: &str, formatter: &crate::formatter::Formatter) -> String {
        let modified = self.apply_autofixes(source, Hook::BeforeFormat);
        let modified = formatter.format(&modified);
        self.apply_autofixes(&modified, Hook::AfterFormat)
    }

    /// Run autofix-enabled rules attached to `hook`, sequentially in
    /// `AUTOFIX_ORDER`.
    fn apply_autofixes(&self, source: &str, hook: Hook) -> String {
        let mut modified = source.to_string();
        for rule in self.fixers(|r| r.hook() == hook) {
            modified = rule.autofix(&modified);
        }
        modified
    }

    /// Run `format_source` against a file. The file is only rewritten if
    /// a byte changed. Returns true when the file was rewritten.
    pub fn format_file(
        &self,
        path: &Path,
        formatter: &crate::formatter::Formatter,
    ) -> Result<bool> {
        let original = encoding::read_source(path)?;
        let modified = self.format_source(&original, formatter);
        if modified.as_bytes() == original.as_bytes() {
            return Ok(false);
        }
        std::fs::write(path, modified.as_bytes())?;
        Ok(true)
    }
}

/// Drop findings that a more specific rule's finding on the same line
/// explains (`rules::REPORT_PRECEDENCE`).
fn apply_report_precedence(findings: Vec<Finding>) -> Vec<Finding> {
    let explained: std::collections::HashSet<(&'static str, u32)> = findings
        .iter()
        .flat_map(|f| {
            rules::REPORT_PRECEDENCE
                .iter()
                .filter(move |(winner, _)| *winner == f.rule)
                .flat_map(move |(_, losers)| losers.iter().map(move |l| (*l, f.line)))
        })
        .collect();
    findings
        .into_iter()
        .filter(|f| !explained.contains(&(f.rule, f.line)))
        .collect()
}
