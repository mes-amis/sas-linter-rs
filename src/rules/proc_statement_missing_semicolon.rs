use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::{Token, TokenStream};

use super::sas::FORMAT_DEFINITIONS;
use super::RuleMeta;

pub struct ProcStatementMissingSemicolon {
    autofix: bool,
}

const ID: &str = "proc_statement_missing_semicolon";
const DESCRIPTION: &str = "`proc` statement missing its terminating `;` — the \
                            statement after it (`value`, `by`, `var`, …) is read \
                            as PROC options.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: true,
        default_factory: || Box::new(ProcStatementMissingSemicolon { autofix: false }),
        config_factory: |opts| {
            Ok(Box::new(ProcStatementMissingSemicolon {
                autofix: crate::config::opt_bool(opts, "autofix").unwrap_or(false),
            }))
        },
    }
}

/// Per PROC, the statement keywords that can't be options of the PROC
/// statement itself — seeing one before the `;` means the `;` is missing.
const PROC_STATEMENTS: &[(&str, &[&str])] = &[
    ("format", FORMAT_DEFINITIONS),
    ("sort", &["by"]),
    ("freq", &["tables", "table"]),
    ("means", &["var", "class"]),
    ("summary", &["var", "class"]),
    ("print", &["var"]),
];

impl Rule for ProcStatementMissingSemicolon {
    fn id(&self) -> &'static str {
        ID
    }
    fn description(&self) -> &'static str {
        DESCRIPTION
    }
    fn supports_autofix(&self) -> bool {
        true
    }
    fn autofix_enabled(&self) -> bool {
        self.autofix
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        offenders(&ctx.tokens.default)
            .into_iter()
            .map(|o| Finding {
                path: ctx.path.to_string(),
                line: o.proc.start_line,
                column: o.proc.start_column + 1,
                rule: ID,
                message: format!(
                    "`proc {}` statement is missing its terminating `;` — the \
                     following `{}` statement is read as PROC options.",
                    o.name.to_lowercase(),
                    o.keyword.text
                ),
                severity: Severity::Warning,
            })
            .collect()
    }

    fn autofix(&self, source: &str) -> String {
        let stream = TokenStream::tokenize(source);
        let mut ends: Vec<usize> = offenders(&stream.default)
            .into_iter()
            .map(|o| o.last_option.end_byte as usize)
            .collect();
        ends.sort_unstable_by(|a, b| b.cmp(a));
        let mut out = source.to_string();
        for end in ends {
            out.insert(end, ';');
        }
        out
    }
}

struct Offender<'a> {
    proc: &'a Token,
    name: &'a str,
    /// The statement keyword swallowed into the PROC statement.
    keyword: &'a Token,
    /// The token just before `keyword`, where the `;` belongs.
    last_option: &'a Token,
}

fn offenders(tokens: &[Token]) -> Vec<Offender<'_>> {
    let mut out = Vec::new();
    for (i, proc) in tokens.iter().enumerate() {
        if proc.token_type != TokenType::KwProc {
            continue;
        }
        let Some(name) = tokens.get(i + 1) else {
            continue;
        };
        let lname = name.text.to_lowercase();
        let Some((_, keywords)) = PROC_STATEMENTS.iter().find(|(p, _)| *p == lname) else {
            continue;
        };
        let mut depth = 0usize;
        for j in i + 2..tokens.len() {
            let t = &tokens[j];
            match t.token_type {
                TokenType::SEMI => break,
                TokenType::LPAREN => depth += 1,
                TokenType::RPAREN => depth = depth.saturating_sub(1),
                _ => {}
            }
            // `data=x(keep=var)` / `cntlin=value` name datasets, not statements.
            let is_option_value = tokens[j - 1].token_type == TokenType::ASSIGN
                || tokens.get(j + 1).map(|n| n.token_type) == Some(TokenType::ASSIGN);
            if depth == 0 && !is_option_value && keywords.contains(&t.text.to_lowercase().as_str())
            {
                out.push(Offender {
                    proc,
                    name: &name.text,
                    keyword: t,
                    last_option: &tokens[j - 1],
                });
                break;
            }
        }
    }
    out
}
