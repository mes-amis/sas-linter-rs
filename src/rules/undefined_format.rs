use std::collections::HashSet;

use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::Token;

use super::RuleMeta;

pub struct UndefinedFormat;

const ID: &str = "undefined_format";
const DESCRIPTION: &str = "`format` / `informat` / `attrib` names a format that the \
                            file's PROC FORMAT doesn't define and SAS doesn't ship — \
                            SAS fails with `ERROR 48-59`.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: false,
        default_factory: || Box::new(UndefinedFormat),
        config_factory: |_| Ok(Box::new(UndefinedFormat)),
    }
}

/// SAS-supplied formats and informats, width digits stripped. Character
/// ones keep their `$`; `$` alone is `$w.`.
const BUILTIN_FORMATS: &[&str] = &[
    // numeric
    "best", "bestd", "binary", "bz", "comma", "commax", "d", "dollar", "dollarx", "e", "euro",
    "eurox", "f", "float", "fract", "hex", "ib", "ibr", "negparen", "nlmny", "nlnum", "nlpct",
    "numx", "octal", "pd", "percent", "percentn", "pib", "pk", "pvalue", "rb", "roman", "s",
    "sizekmg", "ssn", "trailsgn", "wordf", "words", "yen", "z", "zd",
    // date, time, datetime
    "anydtdte", "anydtdtm", "anydttme", "b8601da", "b8601dn", "b8601dt", "b8601dz", "b8601lz",
    "b8601tm", "b8601tz", "date", "dateampm", "datetime", "day", "ddmmyy", "ddmmyyb", "ddmmyyc",
    "ddmmyyd", "ddmmyyn", "ddmmyyp", "ddmmyys", "downame", "dtdate", "dtmonyy", "dtwkdatx",
    "dtyear", "dtyyqc", "e8601da", "e8601dn", "e8601dt", "e8601dz", "e8601lz", "e8601tm",
    "e8601tz", "hhmm", "hour", "is8601da", "is8601dn", "is8601dt", "is8601dz", "is8601lz",
    "is8601tm", "is8601tz", "julday", "julian", "mmddyy", "mmddyyb", "mmddyyc", "mmddyyd",
    "mmddyyn", "mmddyyp", "mmddyys", "mmss", "mmyy", "mmyyc", "mmyyd", "mmyyn", "mmyyp", "mmyys",
    "monname", "month", "monyy", "nengo", "nldate", "nldatm", "nltime", "qtr", "qtrr", "time",
    "timeampm", "tod", "weekdate", "weekdatx", "weekday", "worddate", "worddatx", "year", "yymm",
    "yymmc", "yymmd", "yymmdd", "yymmddb", "yymmddc", "yymmddd", "yymmddn", "yymmddp", "yymmdds",
    "yymmn", "yymmp", "yymms", "yymon", "yyq", "yyqc", "yyqd", "yyqn", "yyqp", "yyqr", "yyqrc",
    "yyqrd", "yyqrn", "yyqrp", "yyqrs", "yyqs", // character
    "$", "$ascii", "$base64x", "$binary", "$char", "$charzb", "$ebcdic", "$f", "$hex", "$msgcase",
    "$octal", "$quote", "$revers", "$reverj", "$upcase", "$varying",
];

impl Rule for UndefinedFormat {
    fn id(&self) -> &'static str {
        ID
    }
    fn description(&self) -> &'static str {
        DESCRIPTION
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        let tokens = &ctx.tokens.default;
        // Without a PROC FORMAT the file's custom formats live in a catalog
        // it can't see; `fmtsearch` / `%include` can pull in more of them.
        let has_proc_format = tokens.windows(2).any(|w| {
            w[0].token_type == TokenType::KwProc && w[1].token_type == TokenType::KwFormat
        });
        let looks_elsewhere = tokens.iter().any(|t| {
            t.token_type == TokenType::KwmInclude || t.text.eq_ignore_ascii_case("fmtsearch")
        });
        if !has_proc_format || looks_elsewhere {
            return Vec::new();
        }

        let defined = defined_formats(tokens);
        let mut findings = Vec::new();
        for stmt in tokens.split(|t| t.token_type == TokenType::SEMI) {
            let Some(opener) = stmt.first() else {
                continue;
            };
            let in_scope: fn(&[Token], usize) -> bool = match opener.token_type {
                TokenType::KwFormat | TokenType::KwInformat => |_, _| true,
                // Only the values of `format=` / `informat=` options.
                TokenType::KwAttrib => |stmt, k| {
                    k >= 2
                        && stmt[k - 1].token_type == TokenType::ASSIGN
                        && matches!(
                            stmt[k - 2].token_type,
                            TokenType::KwFormat | TokenType::KwInformat
                        )
                },
                _ => continue,
            };
            for k in 1..stmt.len() {
                let Some((name, end_byte)) = format_reference(stmt, k) else {
                    continue;
                };
                if !in_scope(stmt, k)
                    || defined.contains(&name)
                    || BUILTIN_FORMATS.contains(&name.as_str())
                {
                    continue;
                }
                let t = &stmt[k];
                findings.push(Finding {
                    path: ctx.path.to_string(),
                    line: t.start_line,
                    column: t.start_column + 1,
                    rule: ID,
                    message: format!(
                        "format `{}` is not defined by this file's PROC FORMAT and is \
                         not a SAS format — SAS fails with `ERROR 48-59`.",
                        &ctx.source[t.start_byte as usize..end_byte as usize]
                    ),
                    severity: Severity::Warning,
                });
            }
        }
        findings
    }
}

/// Names declared by `value` / `invalue` / `picture` statements inside
/// PROC FORMAT, lower-cased, with a leading `$` for character formats.
fn defined_formats(tokens: &[Token]) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut in_proc_format = false;
    for stmt in tokens.split(|t| t.token_type == TokenType::SEMI) {
        let Some(first) = stmt.first() else {
            continue;
        };
        match first.token_type {
            TokenType::KwProc => {
                in_proc_format = stmt.get(1).map(|t| t.token_type) == Some(TokenType::KwFormat);
                continue;
            }
            TokenType::KwData | TokenType::KwRun | TokenType::KwQuit => {
                in_proc_format = false;
                continue;
            }
            _ => {}
        }
        if !in_proc_format
            || first.token_type != TokenType::Identifier
            || !matches!(
                first.text.to_lowercase().as_str(),
                "value" | "invalue" | "picture"
            )
        {
            continue;
        }
        let (dollar, k) = match stmt.get(1) {
            Some(d) if d.token_type == TokenType::DOLLAR => ("$", 2),
            _ => ("", 1),
        };
        if let Some(name) = stmt
            .get(k)
            .filter(|n| n.token_type == TokenType::Identifier)
        {
            out.insert(format!("{}{}", dollar, name.text.to_lowercase()));
        }
    }
    out
}

/// If a format reference starts at `stmt[k]` — `name.`, `name12.2` or a
/// `$name.` token — its normalized name (width stripped) and end byte.
fn format_reference(stmt: &[Token], k: usize) -> Option<(String, u32)> {
    let t = &stmt[k];
    match t.token_type {
        TokenType::CharFormat => {
            let name = t.text.trim_end_matches('.').to_lowercase();
            Some((strip_width(&name), t.end_byte))
        }
        TokenType::Identifier => {
            let next = stmt.get(k + 1)?;
            let is_dot = next.token_type == TokenType::DOT
                || (next.token_type == TokenType::FloatLiteral && next.text.starts_with('.'));
            (is_dot && next.start_byte == t.end_byte)
                .then(|| (strip_width(&t.text.to_lowercase()), next.end_byte))
        }
        _ => None,
    }
}

fn strip_width(name: &str) -> String {
    name.trim_end_matches(|c: char| c.is_ascii_digit())
        .to_string()
}
