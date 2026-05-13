use once_cell::sync::Lazy;
use regex::Regex;
use sas_lexer::{TokenChannel, TokenType};

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};

use super::RuleMeta;

pub struct ChooseOneTemplate;

const ID: &str = "choose_one_template";
const DESCRIPTION: &str = "Source ships with a 'CHOOSE ONE OF THE BELOW STATEMENTS' banner — \
     broken-by-default; consumers must mutate the source to pick a \
     deployment-context guard.";

static BANNER: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)CHOOSE\s+ONE\s+OF\s+THE\s+BELOW\s+STATEMENTS").unwrap());

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: false,
        default_factory: || Box::new(ChooseOneTemplate),
        config_factory: |_| Ok(Box::new(ChooseOneTemplate)),
    }
}

impl Rule for ChooseOneTemplate {
    fn id(&self) -> &'static str {
        ID
    }
    fn description(&self) -> &'static str {
        DESCRIPTION
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        let mut findings = Vec::new();
        for t in &ctx.tokens.all {
            if t.channel != TokenChannel::COMMENT {
                continue;
            }
            if t.token_type != TokenType::PredictedCommentStat {
                continue;
            }
            if !BANNER.is_match(&t.text) {
                continue;
            }
            findings.push(Finding {
                path: ctx.path.to_string(),
                line: t.start_line,
                column: t.start_column + 1,
                rule: ID,
                message: "'CHOOSE ONE OF THE BELOW STATEMENTS' banner — source is \
                          broken-by-default; the alternative validity guards below \
                          are all commented out so every consumer must edit this \
                          file. Pick one variant, delete the others, and remove \
                          this banner."
                    .to_string(),
                severity: Severity::Warning,
            });
        }
        findings
    }
}
