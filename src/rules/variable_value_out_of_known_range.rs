use std::sync::OnceLock;

use sas_lexer::TokenType;

use crate::config::opt_bool;
use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::value_specs::{Spec, SpecKind, SpecSource, ValueSpecs};

use super::condition::{self, CmpOp, Comparison};
use super::RuleMeta;

pub struct VariableValueOutOfKnownRange {
    source: SpecSource,
    autofix: bool,
    specs: OnceLock<ValueSpecs>,
}

const ID: &str = "variable_value_out_of_known_range";
const DESCRIPTION: &str = "Comparison literal falls outside a variable's \
                            documented acceptable values — branch is unreachable.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: false,
        default_factory: || {
            Box::new(VariableValueOutOfKnownRange {
                source: SpecSource::default(),
                autofix: false,
                specs: OnceLock::new(),
            })
        },
        config_factory: |opts| {
            Ok(Box::new(VariableValueOutOfKnownRange {
                source: SpecSource::from_opts(opts)?,
                autofix: opt_bool(opts, "autofix").unwrap_or(false),
                specs: OnceLock::new(),
            }))
        },
    }
}

impl Rule for VariableValueOutOfKnownRange {
    fn id(&self) -> &'static str {
        ID
    }
    fn description(&self) -> &'static str {
        DESCRIPTION
    }
    fn autofix_enabled(&self) -> bool {
        self.autofix
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        let specs = self.specs();
        if specs.is_empty() {
            return Vec::new();
        }
        let tokens = &ctx.tokens.default;
        let mut findings = Vec::new();
        let mut i = 0;
        while i < tokens.len() {
            if tokens[i].token_type != TokenType::KwIf {
                i += 1;
                continue;
            }
            // The condition runs to `then`, or to `;` for a subsetting `if`.
            let end = tokens[i + 1..]
                .iter()
                .position(|t| matches!(t.token_type, TokenType::KwThen | TokenType::SEMI))
                .map_or(tokens.len(), |p| i + 1 + p);
            // Only a comparison that is a whole operand of the condition's
            // `and` / `or` tree tests the variable against the literal:
            // the `x = 5` in `y + x = 5` or `x = 5 + 1` does not.
            for part in condition::logical_operands(&tokens[i + 1..end]) {
                if let Some(cmp) = condition::comparison(part) {
                    findings.extend(self.check_comparison(&cmp, ctx.path, specs));
                }
            }
            i = end;
        }
        findings
    }
}

impl VariableValueOutOfKnownRange {
    fn specs(&self) -> &ValueSpecs {
        self.specs.get_or_init(|| self.source.load())
    }

    /// Findings for the literals of an equality / `in` test that fall
    /// outside the variable's documented values.
    fn check_comparison(&self, cmp: &Comparison, path: &str, specs: &ValueSpecs) -> Vec<Finding> {
        if cmp.op != CmpOp::In {
            return vec![];
        }
        let Some(spec) = specs.lookup(&cmp.var.text) else {
            return vec![];
        };
        cmp.values
            .iter()
            .filter(|v| v.lit.as_f64().is_some_and(|n| !value_allowed(spec, n)))
            .map(|v| Finding {
                path: path.to_string(),
                line: v.tok.start_line,
                column: v.tok.start_column + 1,
                rule: ID,
                message: format_message(&cmp.var.text, &v.display, spec),
                severity: Severity::Warning,
            })
            .collect()
    }
}

/// Is numeric value `n` among the spec's acceptable values?
fn value_allowed(spec: &Spec, n: f64) -> bool {
    let int = n as i64;
    match &spec.kind {
        SpecKind::Decimal(lo, hi) => n >= *lo && n <= *hi,
        // Dates are text; a numeric literal says nothing about them.
        SpecKind::Date => true,
        // Integer catalogs say nothing about fractional literals.
        _ if n.fract() != 0.0 => true,
        SpecKind::Range(lo, hi) => int >= *lo && int <= *hi,
        SpecKind::Set(values) => values.contains(&int),
    }
}

fn format_message(ident: &str, display: &str, spec: &Spec) -> String {
    let allowed_str = match &spec.kind {
        SpecKind::Range(lo, hi) => format!("{}..{}", lo, hi),
        SpecKind::Decimal(lo, hi) => format!("{}..{}", lo, hi),
        SpecKind::Date => "yyyymmdd".to_string(),
        SpecKind::Set(values) => {
            let parts: Vec<String> = values.iter().map(|v| v.to_string()).collect();
            format!("{{{}}}", parts.join(", "))
        }
    };
    format!(
        "value {} for {} is outside the documented acceptable values ({}); \
         this branch is unreachable.",
        display, ident, allowed_str
    )
}
