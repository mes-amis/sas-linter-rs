use once_cell::sync::Lazy;
use regex::Regex;
use sas_lexer::{TokenChannel, TokenType};

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};

use super::RuleMeta;

pub struct CommentedOutGuard;

const ID: &str = "commented_out_guard";
const DESCRIPTION: &str = "SAS `* ... ;` line comment looks like a disabled \
                            `if ... then do` validity guard — review and either \
                            restore the guard or remove the orphan `end;`.";

static GUARD_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)\bif\b.*\bthen\b\s+do\b").unwrap());
static OPENING_STAR: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*\*(?:[^*]|$)").unwrap());

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: false,
        default_factory: || Box::new(CommentedOutGuard),
        config_factory: |_| Ok(Box::new(CommentedOutGuard)),
    }
}

impl Rule for CommentedOutGuard {
    fn id(&self) -> &'static str {
        ID
    }
    fn description(&self) -> &'static str {
        DESCRIPTION
    }

    /// The Ruby version filtered on `TT::COMMENT_STAT`; that variant no
    /// longer exists in sas-lexer 1.0. The replacement is
    /// `PredictedCommentStat`, plus we still require the body to start
    /// with `*` (and NOT `**`) so header-style boxes don't trigger.
    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        let mut findings = Vec::new();
        for t in &ctx.tokens.all {
            if t.channel != TokenChannel::COMMENT {
                continue;
            }
            if t.token_type != TokenType::PredictedCommentStat {
                continue;
            }
            if !OPENING_STAR.is_match(&t.text) {
                continue;
            }
            if !GUARD_PATTERN.is_match(&t.text) {
                continue;
            }
            findings.push(Finding {
                path: ctx.path.to_string(),
                line: t.start_line,
                column: t.start_column + 1,
                rule: ID,
                message: "looks like a disabled validity guard (`* if ... then do; ...`); \
                          review whether the guard should be live or whether the matching \
                          `end;` is now orphaned."
                    .to_string(),
                severity: Severity::Warning,
            });
        }
        findings
    }
}
