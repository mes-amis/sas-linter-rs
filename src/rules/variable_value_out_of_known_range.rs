use std::sync::OnceLock;

use sas_lexer::TokenType;

use crate::config::opt_bool;
use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::Token;
use crate::value_specs::{Spec, SpecKind, SpecSource, ValueSpecs};

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
        let mut in_condition = false;
        let mut i = 0;
        while i < tokens.len() {
            let t = &tokens[i];
            match t.token_type {
                TokenType::KwIf => {
                    in_condition = true;
                    i += 1;
                    continue;
                }
                TokenType::KwThen | TokenType::SEMI => {
                    in_condition = false;
                    i += 1;
                    continue;
                }
                _ => {}
            }
            if in_condition && t.token_type == TokenType::Identifier {
                if let Some(op) = tokens.get(i + 1) {
                    let (consumed, mut sub) =
                        self.check_comparison(tokens, i, t, op, ctx.path, specs);
                    findings.append(&mut sub);
                    if consumed > 0 {
                        i += consumed;
                        continue;
                    }
                }
            }
            i += 1;
        }
        findings
    }
}

impl VariableValueOutOfKnownRange {
    fn specs(&self) -> &ValueSpecs {
        self.specs.get_or_init(|| self.source.load())
    }

    fn check_comparison(
        &self,
        tokens: &[Token],
        ident_idx: usize,
        ident: &Token,
        op: &Token,
        path: &str,
        specs: &ValueSpecs,
    ) -> (usize, Vec<Finding>) {
        let Some(spec) = specs.lookup(&ident.text) else {
            return (0, vec![]);
        };

        match op.token_type {
            TokenType::KwIN => {
                let Some(lparen) = tokens.get(ident_idx + 2) else {
                    return (0, vec![]);
                };
                if lparen.token_type != TokenType::LPAREN {
                    return (0, vec![]);
                }
                let mut findings = Vec::new();
                let mut k = ident_idx + 3;
                while k < tokens.len() {
                    let t = &tokens[k];
                    if t.token_type == TokenType::RPAREN {
                        return (k - ident_idx + 1, findings);
                    }
                    if t.token_type == TokenType::COMMA {
                        k += 1;
                        continue;
                    }
                    if let Some(lit) = literal_value(t) {
                        if !value_allowed(spec, &lit) {
                            findings.push(Finding {
                                path: path.to_string(),
                                line: t.start_line,
                                column: t.start_column + 1,
                                rule: ID,
                                message: format_message(&ident.text, &lit.display, spec),
                                severity: Severity::Warning,
                            });
                        }
                    }
                    k += 1;
                }
                (k - ident_idx + 1, findings)
            }
            TokenType::KwEQ | TokenType::ASSIGN => {
                let Some(lit_tok) = tokens.get(ident_idx + 2) else {
                    return (0, vec![]);
                };
                let Some(lit) = literal_value(lit_tok) else {
                    return (0, vec![]);
                };
                if value_allowed(spec, &lit) {
                    return (3, vec![]);
                }
                (
                    3,
                    vec![Finding {
                        path: path.to_string(),
                        line: lit_tok.start_line,
                        column: lit_tok.start_column + 1,
                        rule: ID,
                        message: format_message(&ident.text, &lit.display, spec),
                        severity: Severity::Warning,
                    }],
                )
            }
            _ => (0, vec![]),
        }
    }
}

#[derive(Debug)]
struct Literal {
    value: f64,
    display: String,
}

fn literal_value(t: &Token) -> Option<Literal> {
    match t.token_type {
        TokenType::IntegerLiteral | TokenType::FloatLiteral => t
            .text
            .parse::<f64>()
            .ok()
            .filter(|f| f.is_finite())
            .map(|value| Literal {
                value,
                display: t.text.clone(),
            }),
        _ => None,
    }
}

fn value_allowed(spec: &Spec, lit: &Literal) -> bool {
    let int = lit.value as i64;
    match &spec.kind {
        SpecKind::Decimal(lo, hi) => lit.value >= *lo && lit.value <= *hi,
        // Integer catalogs say nothing about fractional literals.
        _ if lit.value.fract() != 0.0 => true,
        SpecKind::Range(lo, hi) => int >= *lo && int <= *hi,
        SpecKind::Set(values) => values.contains(&int),
    }
}

fn format_message(ident: &str, display: &str, spec: &Spec) -> String {
    let allowed_str = match &spec.kind {
        SpecKind::Range(lo, hi) => format!("{}..{}", lo, hi),
        SpecKind::Decimal(lo, hi) => format!("{}..{}", lo, hi),
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
