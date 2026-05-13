use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use once_cell::sync::Lazy;
use regex::Regex;
use sas_lexer::TokenType;

use crate::config::{opt_bool, opt_seq_str, opt_str};
use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::Token;

use super::RuleMeta;

pub struct VariableValueOutOfKnownRange {
    csv_paths: Vec<PathBuf>,
    name_column: String,
    values_column: String,
    name_match: NameMatch,
    delimiter: u8,
    autofix: bool,
    specs: OnceLock<HashMap<String, Spec>>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum NameMatch {
    CaseInsensitive,
    Exact,
}

#[derive(Debug, Clone)]
enum SpecKind {
    Range(i64, i64),
    Set(Vec<i64>),
}

#[derive(Debug, Clone)]
struct Spec {
    kind: SpecKind,
}

const ID: &str = "variable_value_out_of_known_range";
const DESCRIPTION: &str = "Comparison literal falls outside a variable's \
                            documented acceptable values — branch is unreachable.";
const DEFAULT_NAME_COLUMN: &str = "Variable";
const DEFAULT_VALUES_COLUMN: &str = "Acceptable Values";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: false,
        default_factory: || {
            Box::new(VariableValueOutOfKnownRange {
                csv_paths: Vec::new(),
                name_column: DEFAULT_NAME_COLUMN.into(),
                values_column: DEFAULT_VALUES_COLUMN.into(),
                name_match: NameMatch::CaseInsensitive,
                delimiter: b',',
                autofix: false,
                specs: OnceLock::new(),
            })
        },
        config_factory: |opts| {
            let csv_paths = opt_seq_str(opts, "csv_paths")
                .into_iter()
                .map(PathBuf::from)
                .collect();
            let name_match = match opt_str(opts, "name_match").unwrap_or("case_insensitive") {
                "exact" => NameMatch::Exact,
                "case_insensitive" => NameMatch::CaseInsensitive,
                other => anyhow::bail!(
                    "name_match must be 'case_insensitive' or 'exact' (got {:?})",
                    other
                ),
            };
            let delimiter = opt_str(opts, "delimiter").unwrap_or(",");
            let delimiter = delimiter.as_bytes().first().copied().unwrap_or(b',');
            Ok(Box::new(VariableValueOutOfKnownRange {
                csv_paths,
                name_column: opt_str(opts, "name_column")
                    .unwrap_or(DEFAULT_NAME_COLUMN)
                    .into(),
                values_column: opt_str(opts, "values_column")
                    .unwrap_or(DEFAULT_VALUES_COLUMN)
                    .into(),
                name_match,
                delimiter,
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
    fn specs(&self) -> &HashMap<String, Spec> {
        self.specs.get_or_init(|| self.load_specs())
    }

    fn load_specs(&self) -> HashMap<String, Spec> {
        let mut map = HashMap::new();
        for path in &self.csv_paths {
            if !path.is_file() {
                continue;
            }
            let Ok(mut rdr) = csv::ReaderBuilder::new()
                .delimiter(self.delimiter)
                .has_headers(true)
                .from_path(path)
            else {
                continue;
            };
            let headers = match rdr.headers() {
                Ok(h) => h.clone(),
                Err(_) => continue,
            };
            let name_col = headers.iter().position(|h| h == self.name_column);
            let values_col = headers.iter().position(|h| h == self.values_column);
            let (Some(name_col), Some(values_col)) = (name_col, values_col) else {
                continue;
            };
            for record in rdr.records().flatten() {
                let name = record.get(name_col).unwrap_or("").trim();
                let values_text = record.get(values_col).unwrap_or("").trim();
                if name.is_empty() || values_text.is_empty() {
                    continue;
                }
                let Some(spec) = parse_values(values_text) else {
                    continue;
                };
                let key = match self.name_match {
                    NameMatch::Exact => name.to_string(),
                    NameMatch::CaseInsensitive => name.to_lowercase(),
                };
                map.insert(key, spec);
            }
        }
        map
    }

    fn lookup<'a>(&self, text: &str, specs: &'a HashMap<String, Spec>) -> Option<&'a Spec> {
        let key = match self.name_match {
            NameMatch::Exact => text.to_string(),
            NameMatch::CaseInsensitive => text.to_lowercase(),
        };
        specs.get(&key)
    }

    fn check_comparison(
        &self,
        tokens: &[Token],
        ident_idx: usize,
        ident: &Token,
        op: &Token,
        path: &str,
        specs: &HashMap<String, Spec>,
    ) -> (usize, Vec<Finding>) {
        let Some(spec) = self.lookup(&ident.text, specs) else {
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
    value: i64,
    display: String,
}

fn literal_value(t: &Token) -> Option<Literal> {
    match t.token_type {
        TokenType::IntegerLiteral => t.text.parse::<i64>().ok().map(|n| Literal {
            value: n,
            display: t.text.clone(),
        }),
        TokenType::FloatLiteral => t.text.parse::<f64>().ok().and_then(|f| {
            if f == f.trunc() && f.is_finite() {
                Some(Literal {
                    value: f as i64,
                    display: t.text.clone(),
                })
            } else {
                None
            }
        }),
        _ => None,
    }
}

fn value_allowed(spec: &Spec, lit: &Literal) -> bool {
    match &spec.kind {
        SpecKind::Range(lo, hi) => lit.value >= *lo && lit.value <= *hi,
        SpecKind::Set(values) => values.contains(&lit.value),
    }
}

static RE_RANGE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(-?\d+)-(-?\d+)$").unwrap());
static RE_SET: Lazy<Regex> = Lazy::new(|| Regex::new(r"^-?\d+(?:\s*,\s*-?\d+)+$").unwrap());
static RE_RANGE_PAREN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(\d+)-(\d+)\s+\(([^)]+)\)$").unwrap());
static RE_RANGE_PLUS: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(\d+)-(\d+)\s*,\s*(.+)$").unwrap());

fn parse_values(text: &str) -> Option<Spec> {
    if let Some(c) = RE_RANGE.captures(text) {
        let lo: i64 = c[1].parse().ok()?;
        let hi: i64 = c[2].parse().ok()?;
        return Some(Spec {
            kind: SpecKind::Range(lo, hi),
        });
    }
    if RE_SET.is_match(text) {
        let vals: Vec<i64> = text
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        return Some(Spec {
            kind: SpecKind::Set(vals),
        });
    }
    if let Some(c) = RE_RANGE_PAREN.captures(text) {
        let lo: i64 = c[1].parse().ok()?;
        let hi: i64 = c[2].parse().ok()?;
        let mut vals: Vec<i64> = (lo..=hi).collect();
        for x in c[3].split(',') {
            if let Ok(n) = x.trim().parse::<i64>() {
                vals.push(n);
            }
        }
        return Some(Spec {
            kind: SpecKind::Set(vals),
        });
    }
    if let Some(c) = RE_RANGE_PLUS.captures(text) {
        let lo: i64 = c[1].parse().ok()?;
        let hi: i64 = c[2].parse().ok()?;
        let extras: Vec<i64> = c[3]
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        if extras.is_empty() {
            return None;
        }
        let mut vals: Vec<i64> = (lo..=hi).collect();
        vals.extend(extras);
        return Some(Spec {
            kind: SpecKind::Set(vals),
        });
    }
    None
}

fn format_message(ident: &str, display: &str, spec: &Spec) -> String {
    let allowed_str = match &spec.kind {
        SpecKind::Range(lo, hi) => format!("{}..{}", lo, hi),
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
