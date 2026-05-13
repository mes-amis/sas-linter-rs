use anyhow::Result;
use std::path::Path;

use crate::config::Config;
use crate::encoding;
use crate::finding::Finding;
use crate::rule::{CheckContext, Rule};
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

        let mut modified = source.to_string();
        for rule in &self.rules {
            if rule.autofix_enabled() && rule.supports_autofix() {
                modified = rule.autofix(&modified);
            }
        }

        (findings, modified)
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

    /// Run the formatter against a file, then apply any autofix-enabled
    /// rules. Mirrors the Ruby gem's `format_file`: formatter passes
    /// first, then composable rule autofixes, and the file is only
    /// rewritten if a byte changed. Returns true when the file was
    /// rewritten.
    pub fn format_file(
        &self,
        path: &Path,
        formatter: &crate::formatter::Formatter,
    ) -> Result<bool> {
        let original = encoding::read_source(path)?;
        let mut modified = formatter.format(&original);
        for rule in &self.rules {
            if rule.autofix_enabled() && rule.supports_autofix() {
                modified = rule.autofix(&modified);
            }
        }
        if modified.as_bytes() == original.as_bytes() {
            return Ok(false);
        }
        std::fs::write(path, modified.as_bytes())?;
        Ok(true)
    }
}
