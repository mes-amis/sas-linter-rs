use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::{Token, TokenStream};

use super::sas::FORMAT_DEFINITIONS;
use super::RuleMeta;

pub struct OrphanFormatRange {
    autofix: bool,
}

const ID: &str = "orphan_format_range";
const DESCRIPTION: &str = "PROC FORMAT range left as a statement of its own after a \
                            stray `;` closed the `value` / `invalue` / `picture` \
                            statement early — SAS fails with `ERROR 180-322`.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: true,
        default_factory: || Box::new(OrphanFormatRange { autofix: false }),
        config_factory: |opts| {
            Ok(Box::new(OrphanFormatRange {
                autofix: crate::config::opt_bool(opts, "autofix").unwrap_or(false),
            }))
        },
    }
}

impl Rule for OrphanFormatRange {
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
        orphans(&ctx.tokens.default)
            .into_iter()
            .map(|o| Finding {
                path: ctx.path.to_string(),
                line: o.first.start_line,
                column: o.first.start_column + 1,
                rule: ID,
                message: format!(
                    "range is outside the `{}` statement — the `;` at line {} ends \
                     it early, so SAS reads this as a statement of its own.",
                    o.owner, o.early_semi.start_line
                ),
                severity: Severity::Warning,
            })
            .collect()
    }

    fn autofix(&self, source: &str) -> String {
        let stream = TokenStream::tokenize(source);
        let mut semis: Vec<usize> = orphans(&stream.default)
            .into_iter()
            .map(|o| o.early_semi.start_byte as usize)
            .collect();
        semis.sort_unstable_by(|a, b| b.cmp(a));
        let mut out = source.to_string();
        for at in semis {
            out.remove(at);
        }
        out
    }
}

struct Orphan<'a> {
    first: &'a Token,
    /// `value levels` — the statement the range belongs to.
    owner: String,
    /// The `;` that closed the owning statement (or the previous orphan).
    early_semi: &'a Token,
}

fn orphans(tokens: &[Token]) -> Vec<Orphan<'_>> {
    let mut out = Vec::new();
    let mut in_proc_format = false;
    // The `value …` statement whose ranges a following orphan belongs to.
    let mut owner: Option<String> = None;
    let mut start = 0;
    for (i, t) in tokens.iter().enumerate() {
        if t.token_type != TokenType::SEMI {
            continue;
        }
        let stmt = &tokens[start..i];
        let prev_semi = start.checked_sub(1).map(|p| &tokens[p]);
        start = i + 1;
        let Some(first) = stmt.first() else {
            continue;
        };
        match first.token_type {
            TokenType::KwProc => {
                in_proc_format = stmt.get(1).map(|n| n.token_type) == Some(TokenType::KwFormat);
                owner = None;
                continue;
            }
            TokenType::KwRun | TokenType::KwQuit | TokenType::KwData => {
                in_proc_format = false;
                owner = None;
                continue;
            }
            _ => {}
        }
        if !in_proc_format {
            continue;
        }
        if let Some(name) = definition_header(stmt) {
            owner = Some(name);
        } else if let (true, Some(o), Some(semi)) = (is_range(stmt), &owner, prev_semi) {
            out.push(Orphan {
                first,
                owner: o.clone(),
                early_semi: semi,
            });
        } else {
            owner = None;
        }
    }
    out
}

/// `value levels` / `invalue $x` / `picture p` for a definition statement.
fn definition_header(stmt: &[Token]) -> Option<String> {
    let kw = stmt.first()?;
    let kw_lower = kw.text.to_lowercase();
    if kw.token_type != TokenType::Identifier || !FORMAT_DEFINITIONS.contains(&kw_lower.as_str()) {
        return None;
    }
    let mut name = String::new();
    for t in stmt[1..].iter().take(2) {
        name.push_str(&t.text);
        if t.token_type != TokenType::DOLLAR {
            break;
        }
    }
    Some(format!("{} {}", kw_lower, name))
}

/// A statement shaped like a format range: a literal, `low` / `high` /
/// `other` or a negative number, followed somewhere by `=`.
fn is_range(stmt: &[Token]) -> bool {
    let starts_like_range = match stmt[0].token_type {
        TokenType::IntegerLiteral | TokenType::FloatLiteral | TokenType::StringLiteral => true,
        TokenType::MINUS => matches!(
            stmt.get(1).map(|t| t.token_type),
            Some(TokenType::IntegerLiteral | TokenType::FloatLiteral)
        ),
        TokenType::Identifier => matches!(
            stmt[0].text.to_lowercase().as_str(),
            "low" | "high" | "other"
        ),
        _ => false,
    };
    starts_like_range && stmt.iter().any(|t| t.token_type == TokenType::ASSIGN)
}
