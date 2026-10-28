use std::collections::HashSet;

use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::Token;

use super::{invalid_assignment_target, RuleMeta};

pub struct StatementNotValid;

const ID: &str = "statement_not_valid";
const DESCRIPTION: &str = "Statement in open code or a data step that doesn't start \
                            like any SAS statement — bare text, stray punctuation, a \
                            comment that leaked into code. SAS fails with \
                            `ERROR 180-322`.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: false,
        default_factory: || Box::new(StatementNotValid),
        config_factory: |_| Ok(Box::new(StatementNotValid)),
    }
}

/// Statement names the lexer emits as plain identifiers rather than `Kw*`
/// tokens.
const STATEMENT_WORDS: &[&str] = &[
    "abort",
    "catname",
    "cards",
    "continue",
    "datalines",
    "dcl",
    "display",
    "dm",
    "endrsubmit",
    "endsas",
    "error",
    "file",
    "go",
    "goptions",
    "goto",
    "infile",
    "leave",
    "lines",
    "link",
    "lock",
    "lostcard",
    "modify",
    "ods",
    "options",
    "otherwise",
    "page",
    "putlog",
    "remove",
    "replace",
    "resetline",
    "retain",
    "return",
    "rsubmit",
    "sasfile",
    "signoff",
    "signon",
    "skip",
    "systask",
    "waitfor",
    "window",
    "x",
];

/// Statements that also take a number suffix: `title2`, `symbol1`, ….
const NUMBERED_STATEMENT_WORDS: &[&str] =
    &["axis", "footnote", "legend", "pattern", "symbol", "title"];

/// Keyword tokens a data-step or global statement can start with. The
/// lexer also has keyword types for SQL and option words (`from`, `as`,
/// `then`, …) that never start one.
const STATEMENT_KEYWORDS: &[TokenType] = &[
    TokenType::KwArray,
    TokenType::KwAttrib,
    TokenType::KwBy,
    TokenType::KwCall,
    TokenType::KwData,
    TokenType::KwDeclare,
    TokenType::KwDelete,
    TokenType::KwDo,
    TokenType::KwDrop,
    TokenType::KwElse,
    TokenType::KwEnd,
    TokenType::KwFilename,
    TokenType::KwFormat,
    TokenType::KwIf,
    TokenType::KwInfile,
    TokenType::KwInformat,
    TokenType::KwInput,
    TokenType::KwKeep,
    TokenType::KwLabel,
    TokenType::KwLength,
    TokenType::KwLibname,
    TokenType::KwList,
    TokenType::KwMerge,
    TokenType::KwMissing,
    TokenType::KwOutput,
    TokenType::KwProc,
    TokenType::KwPut,
    TokenType::KwQuit,
    TokenType::KwRename,
    TokenType::KwRun,
    TokenType::KwSelect,
    TokenType::KwSet,
    TokenType::KwStop,
    TokenType::KwUpdate,
    TokenType::KwWhen,
    TokenType::KwWhere,
];

/// PROCs that keep accepting statements after `run;` until `quit;`.
const RUN_GROUP_PROCS: &[&str] = &[
    "anova", "arima", "catalog", "datasets", "ds2", "fedsql", "gchart", "glm", "gmap", "gplot",
    "iml", "optmodel", "plm", "plot", "reg", "sql",
];

/// Where a statement sits: PROC bodies have per-PROC statements and aren't
/// checked; open code and data steps are. A run-group PROC's body only
/// ends at `quit;`.
#[derive(PartialEq)]
enum Scope {
    Checked,
    Proc { run_group: bool },
}

impl Rule for StatementNotValid {
    fn id(&self) -> &'static str {
        ID
    }
    fn description(&self) -> &'static str {
        DESCRIPTION
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        let mut findings = Vec::new();
        let mut scope = Scope::Checked;
        let tokens = &ctx.tokens.default;
        let tokens = match tokens.last() {
            Some(t) if t.token_type == TokenType::EOF => &tokens[..tokens.len() - 1],
            _ => tokens,
        };
        // Statements `invalid_assignment_target` reports, keyed by the
        // first token's offset; it knows the fix.
        let multi_word_targets: HashSet<u32> = invalid_assignment_target::offenders(tokens)
            .iter()
            .map(|run| run[0].start_byte)
            .collect();
        for stmt in tokens.split(|t| t.token_type == TokenType::SEMI) {
            let Some(first) = stmt.first() else {
                continue;
            };
            match first.token_type {
                TokenType::KwProc => {
                    let name = stmt
                        .get(1)
                        .map(|t| t.text.to_lowercase())
                        .unwrap_or_default();
                    scope = Scope::Proc {
                        run_group: RUN_GROUP_PROCS.contains(&name.as_str()),
                    };
                    continue;
                }
                TokenType::KwData | TokenType::KwQuit => scope = Scope::Checked,
                TokenType::KwRun if scope == (Scope::Proc { run_group: false }) => {
                    scope = Scope::Checked;
                    continue;
                }
                _ => {}
            }
            if matches!(scope, Scope::Proc { .. })
                || stmt.iter().any(is_macro)
                || starts_validly(stmt)
                || multi_word_targets.contains(&first.start_byte)
            {
                continue;
            }
            findings.push(Finding {
                path: ctx.path.to_string(),
                line: first.start_line,
                column: first.start_column + 1,
                rule: ID,
                message: format!(
                    "`{}` is not a valid SAS statement — SAS fails with \
                     `ERROR 180-322: Statement is not valid or it is used out of \
                     proper order`.",
                    excerpt(ctx.source, first)
                ),
                severity: Severity::Warning,
            });
        }
        findings
    }
}

fn is_macro(t: &Token) -> bool {
    matches!(
        t.token_type,
        TokenType::AMP | TokenType::PERCENT | TokenType::MacroSep
    ) || (TokenType::MacroComment..=TokenType::KwmRun).contains(&t.token_type)
}

fn starts_validly(stmt: &[Token]) -> bool {
    let first = &stmt[0];
    let next = stmt.get(1).map(|t| t.token_type);
    match first.token_type {
        ty if STATEMENT_KEYWORDS.contains(&ty) => true,
        TokenType::DatalinesStart | TokenType::DatalinesData => true,
        TokenType::NameLiteral => next == Some(TokenType::ASSIGN),
        TokenType::Identifier => {
            let shaped = match next {
                // assignment, array element, sum statement, statement label
                Some(
                    TokenType::ASSIGN
                    | TokenType::LPAREN
                    | TokenType::LBRACK
                    | TokenType::LCURLY
                    | TokenType::PLUS
                    | TokenType::COLON,
                ) => true,
                // hash / hiter method call: `h.add();`
                Some(TokenType::DOT) => {
                    stmt.get(2).map(|t| t.token_type) == Some(TokenType::Identifier)
                        && stmt.get(3).map(|t| t.token_type) == Some(TokenType::LPAREN)
                }
                _ => false,
            };
            shaped || is_statement_word(&first.text)
        }
        _ => false,
    }
}

fn is_statement_word(text: &str) -> bool {
    let word = text.to_lowercase();
    if STATEMENT_WORDS.contains(&word.as_str()) {
        return true;
    }
    let base = word.trim_end_matches(|c: char| c.is_ascii_digit());
    NUMBERED_STATEMENT_WORDS.contains(&base)
}

/// The statement's text on its first line, shortened for the message.
fn excerpt(source: &str, first: &Token) -> String {
    let rest = &source[first.start_byte as usize..];
    let line = rest.lines().next().unwrap_or("").trim_end();
    let line = line.strip_suffix(';').unwrap_or(line);
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= 40 {
        line.to_string()
    } else {
        format!("{}...", chars[..37].iter().collect::<String>())
    }
}
