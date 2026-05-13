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
    fn check(&self, ctx: &CheckContext) -> Vec<Finding>;
    fn autofix(&self, source: &str) -> String {
        source.to_string()
    }
}
