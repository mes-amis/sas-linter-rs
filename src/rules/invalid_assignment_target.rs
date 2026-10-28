use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::{Token, TokenStream};

use super::RuleMeta;

pub struct InvalidAssignmentTarget {
    autofix: bool,
}

const ID: &str = "invalid_assignment_target";
const DESCRIPTION: &str = "Assignment target is several space-separated words \
                            (`Predicted risk = ...`) — SAS variable names cannot \
                            contain spaces. Autofix joins the words with `_`.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: true,
        default_factory: || Box::new(InvalidAssignmentTarget { autofix: false }),
        config_factory: |opts| {
            Ok(Box::new(InvalidAssignmentTarget {
                autofix: crate::config::opt_bool(opts, "autofix").unwrap_or(false),
            }))
        },
    }
}

/// Data-step statement keywords that sas-lexer lexes as plain `Identifier`
/// (no dedicated `Kw*` token type). A statement opening with one of these
/// is a keyword statement whose options may legitimately look like
/// `IDENT IDENT = value` — e.g. `file log linesize = 80;` — so it must
/// not be read as a botched assignment.
const STATEMENT_HEAD_IDENTS: &[&str] = &[
    "abort",
    "continue",
    "display",
    "dm",
    "error",
    "file",
    "footnote",
    "goptions",
    "goto",
    "leave",
    "link",
    "list",
    "lostcard",
    "modify",
    "options",
    "otherwise",
    "putlog",
    "redirect",
    "remove",
    "retain",
    "return",
    "skip",
    "title",
    "window",
];

impl Rule for InvalidAssignmentTarget {
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
            .map(|run| {
                let first = run[0];
                Finding {
                    path: ctx.path.to_string(),
                    line: first.start_line,
                    column: first.start_column + 1,
                    rule: ID,
                    message: format!(
                        "`{}` is not a valid assignment target — SAS variable names \
                         cannot contain spaces. Did you mean `{}`?",
                        spaced_name(&run),
                        joined_name(&run),
                    ),
                    severity: Severity::Warning,
                }
            })
            .collect()
    }

    fn autofix(&self, source: &str) -> String {
        if source.is_empty() {
            return source.to_string();
        }
        let stream = TokenStream::tokenize(source);
        let mut runs = offenders(&stream.default);
        // Splice right-to-left so earlier byte offsets stay valid.
        runs.sort_by(|a, b| b[0].start_byte.cmp(&a[0].start_byte));

        let mut fixed = source.to_string();
        for run in runs {
            let start = run[0].start_byte as usize;
            let end = run[run.len() - 1].end_byte as usize;
            if start >= end || end > fixed.len() {
                continue;
            }
            fixed.replace_range(start..end, &joined_name(&run));
        }
        fixed
    }
}

fn spaced_name(run: &[&Token]) -> String {
    run.iter()
        .map(|t| t.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn joined_name(run: &[&Token]) -> String {
    run.iter()
        .map(|t| t.text.as_str())
        .collect::<Vec<_>>()
        .join("_")
}

/// Scan for statements that open with 2+ consecutive `Identifier` tokens
/// followed directly by `=`. Only inside a data step — proc steps have
/// many identifier-led statements with that exact shape (`model y = x;`,
/// `weight w = ...;`), and open code has `options ls = 80;`.
///
/// Statement keywords in data steps carry dedicated `Kw*` token types
/// (`drop`, `label`, `rename`, `format`, ...), so a statement whose first
/// token is a plain `Identifier` can only be an assignment, a sum
/// statement, or one of the few keyword statements the lexer doesn't
/// special-case (see `STATEMENT_HEAD_IDENTS`).
fn offenders(tokens: &[Token]) -> Vec<Vec<&Token>> {
    let mut out = Vec::new();
    let mut in_data_step = false;
    let mut stmt_start = true;
    let mut i = 0;

    while i < tokens.len() {
        let t = &tokens[i];
        let ty = t.token_type;

        if stmt_start {
            match ty {
                TokenType::KwData => in_data_step = true,
                TokenType::KwProc
                | TokenType::KwRun
                | TokenType::KwQuit
                | TokenType::KwmMacro
                | TokenType::KwmMend => in_data_step = false,
                TokenType::Identifier if in_data_step => {
                    let mut j = i;
                    while j < tokens.len() && tokens[j].token_type == TokenType::Identifier {
                        j += 1;
                    }
                    let run: Vec<&Token> = tokens[i..j].iter().collect();
                    let head_is_keyword = STATEMENT_HEAD_IDENTS
                        .iter()
                        .any(|kw| run[0].text.eq_ignore_ascii_case(kw));
                    if run.len() >= 2
                        && !head_is_keyword
                        && tokens.get(j).map(|n| n.token_type) == Some(TokenType::ASSIGN)
                    {
                        out.push(run);
                    }
                    stmt_start = false;
                    i = j;
                    continue;
                }
                _ => {}
            }
            stmt_start = false;
        }

        if matches!(ty, TokenType::SEMI | TokenType::KwThen | TokenType::KwElse) {
            stmt_start = true;
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::TokenStream;

    fn check(src: &str) -> Vec<Finding> {
        let stream = TokenStream::tokenize(src);
        let rule = InvalidAssignmentTarget { autofix: false };
        rule.check(&CheckContext {
            tokens: &stream,
            source: src,
            path: "test.sas",
        })
    }

    fn fix(src: &str) -> String {
        InvalidAssignmentTarget { autofix: true }.autofix(src)
    }

    #[test]
    fn fires_on_spaced_assignment_target() {
        let src = "data one;\n  Predicted risk = exp(score) / (1 + exp(score));\nrun;\n";
        let findings = check(src);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, 2);
        assert_eq!(findings[0].column, 3);
        assert!(findings[0].message.contains("`Predicted risk`"));
        assert!(findings[0].message.contains("`Predicted_risk`"));
    }

    #[test]
    fn fires_after_then_keyword() {
        let src = "data one;\n  if flag = 1 then Predicted risk = 0.5;\nrun;\n";
        let findings = check(src);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("`Predicted_risk`"));
    }

    #[test]
    fn joins_three_word_targets() {
        let src = "data one;\n  my long name = 1;\nrun;\n";
        let findings = check(src);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("`my_long_name`"));
    }

    #[test]
    fn silent_on_valid_assignment() {
        let src = "data one;\n  Predicted_risk = exp(score) / (1 + exp(score));\nrun;\n";
        assert!(check(src).is_empty());
    }

    #[test]
    fn silent_on_proc_statements() {
        let src = "proc reg data=one;\n  model risk = age gender;\nrun;\n";
        assert!(check(src).is_empty());
    }

    #[test]
    fn silent_on_open_code_options() {
        let src = "options linesize = 80;\ndata one;\n  x = 1;\nrun;\n";
        assert!(check(src).is_empty());
    }

    #[test]
    fn silent_on_file_statement_options() {
        let src = "data one;\n  file log linesize = 80;\n  x = 1;\nrun;\n";
        assert!(check(src).is_empty());
    }

    #[test]
    fn silent_on_keyword_led_statements() {
        let src = "data one;\n  label risk = 'Predicted risk';\n  rename old = new;\nrun;\n";
        assert!(check(src).is_empty());
    }

    #[test]
    fn autofix_joins_words_with_underscores() {
        let src = "data one;\n  Predicted risk = exp(score) / (1 + exp(score));\nrun;\n";
        let fixed = fix(src);
        assert!(fixed.contains("Predicted_risk = exp(score)"));
        assert!(!fixed.contains("Predicted risk"));
    }

    #[test]
    fn autofix_leaves_clean_source_untouched() {
        let src = "data one;\n  Predicted_risk = 1;\nrun;\n";
        assert_eq!(fix(src), src);
    }
}
