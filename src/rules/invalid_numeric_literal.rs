use once_cell::sync::Lazy;
use regex::Regex;
use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};

use super::RuleMeta;

pub struct InvalidNumericLiteral;

const ID: &str = "invalid_numeric_literal";
const DESCRIPTION: &str =
    "INTEGER_LITERAL must be a plain decimal or a SAS hex literal (`0FFx`-style); \
     reject suffixes like `1f` or `1d2` that the lexer accepts but SAS does not.";

static VALID: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(?:[0-9]+|[0-9][0-9A-Fa-f]*[xX])$").unwrap());

const MESSAGE_SUFFIX: &str = "is not a valid SAS numeric literal — SAS has no \
                              `f`/`F`/`L` numeric suffixes and uses `E` (not `D`) \
                              for exponents; hex literals must end in `x`/`X`.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: false,
        default_factory: || Box::new(InvalidNumericLiteral),
        config_factory: |_| Ok(Box::new(InvalidNumericLiteral)),
    }
}

impl Rule for InvalidNumericLiteral {
    fn id(&self) -> &'static str {
        ID
    }
    fn description(&self) -> &'static str {
        DESCRIPTION
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        let mut findings = Vec::new();
        for t in &ctx.tokens.default {
            if t.token_type != TokenType::IntegerLiteral {
                continue;
            }
            if VALID.is_match(&t.text) {
                continue;
            }
            findings.push(Finding {
                path: ctx.path.to_string(),
                line: t.start_line,
                column: t.start_column + 1,
                rule: ID,
                message: format!("`{}` {}", t.text, MESSAGE_SUFFIX),
                severity: Severity::Warning,
            });
        }
        findings
    }
}
