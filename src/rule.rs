use crate::finding::Finding;
use crate::token::TokenStream;

/// Context passed to each rule's `check`. Mirrors the four kwargs the
/// Ruby base class accepted: tokens (default-channel), path (for finding
/// location), all_tokens (raw lexer output), source (raw text).
pub struct CheckContext<'a> {
    pub tokens: &'a TokenStream,
    pub source: &'a str,
    pub path: &'a str,
}

/// Autofix phase relative to the formatter (`hook:` config option).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hook {
    BeforeFormat,
    AfterFormat,
}

impl Hook {
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        match s {
            "before_format" => Ok(Hook::BeforeFormat),
            "after_format" => Ok(Hook::AfterFormat),
            other => anyhow::bail!(
                "hook must be 'before_format' or 'after_format' (got '{}')",
                other
            ),
        }
    }
}

/// Rule lifecycle:
///
/// * `id` / `description` / `severity` / `supports_autofix` are class-level
///   metadata in Ruby; here they're trait methods. Constants are fine —
///   each impl is for one concrete struct.
/// * `check` does the work; returns zero-or-more `Finding`s.
/// * `autofix_enabled` is per-instance (config-driven). `autofix` rewrites
///   source — only runs when both `supports_autofix` and `autofix_enabled`
///   are true.
pub trait Rule: Send + Sync {
    fn id(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn severity(&self) -> crate::finding::Severity {
        crate::finding::Severity::Warning
    }
    fn supports_autofix(&self) -> bool {
        false
    }
    fn autofix_enabled(&self) -> bool {
        false
    }
    /// When this rule's autofix runs in `--format` mode. Byte-level
    /// normalizations (encoding, line endings, tabs) default to
    /// `BeforeFormat`: the formatter measures columns and splits lines,
    /// so fixing them afterwards leaves its output unsettled until a
    /// second save. Overridable per rule via `hook:` in the config.
    fn hook(&self) -> Hook {
        Hook::AfterFormat
    }
    fn check(&self, ctx: &CheckContext) -> Vec<Finding>;
    fn autofix(&self, source: &str) -> String {
        source.to_string()
    }
}
