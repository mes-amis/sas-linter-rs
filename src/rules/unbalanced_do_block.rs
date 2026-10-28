use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::Token;

use super::RuleMeta;

pub struct UnbalancedDoBlock;

const ID: &str = "unbalanced_do_block";
const DESCRIPTION: &str = "`do` / `end` nesting doesn't balance — an unclosed \
                            `do` (reported at the `do` that never closes) or a \
                            stray `end` with no open block.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        // Deliberately report-only: where a missing `end` belongs is a
        // semantic call. Appending one at EOF parses, but silently changes
        // which statements are conditional — a plausible-looking wrong
        // placement is worse than the error.
        supports_autofix: false,
        default_factory: || Box::new(UnbalancedDoBlock),
        config_factory: |_| Ok(Box::new(UnbalancedDoBlock)),
    }
}

/// What put a frame on the stack. Each is closed by a single `end`.
#[derive(Clone, Copy)]
enum Opener {
    Do,
    Select,
    /// PROC SQL `case … end` — an expression, so its `end` needn't end the
    /// statement.
    Case,
    /// PROC TEMPLATE `define …; … end;` / `edit …; … end;`.
    Define,
    Edit,
    /// PROC DS2 `method …; … end;`.
    Method,
}

impl Opener {
    fn keyword(self) -> &'static str {
        match self {
            Opener::Do => "do",
            Opener::Select => "select",
            Opener::Case => "case",
            Opener::Define => "define",
            Opener::Edit => "edit",
            Opener::Method => "method",
        }
    }
}

/// Which block grammar the current step uses. `end` closes something
/// different in each, so the step has to be known before an `end` can be
/// matched.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StepKind {
    /// A data step, open code (fragments start here), or a PROC whose
    /// programming statements nest `do` / `select` exactly like a data step.
    DataStep,
    Sql,
    Template,
    Ds2,
    /// Any other PROC: no `do` / `end` model, so nothing inside is counted.
    /// Embedded-language PROCs (LUA, PYTHON, …) carry foreign `do` / `end`.
    Opaque,
}

impl StepKind {
    fn for_proc(name: &str) -> StepKind {
        match name.to_ascii_lowercase().as_str() {
            "sql" => StepKind::Sql,
            "template" => StepKind::Template,
            "ds2" => StepKind::Ds2,
            "fcmp" | "iml" | "report" | "optmodel" | "model" | "mcmc" | "nlmixed" | "nlin"
            | "glimmix" | "genmod" | "phreg" => StepKind::DataStep,
            _ => StepKind::Opaque,
        }
    }

    fn stray_end_message(self) -> &'static str {
        match self {
            StepKind::Sql => "`end` closes nothing — no `case` expression is open here.",
            StepKind::Template => {
                "`end` closes nothing — no `define` or `edit` block is open here."
            }
            StepKind::Ds2 => {
                "`end` closes nothing — no `do`, `select` or `method` block is open here."
            }
            StepKind::DataStep | StepKind::Opaque => {
                "`end` closes nothing — no `do` or `select` block is open here."
            }
        }
    }
}

/// A `%do … %end` or `%macro … %mend` body. Whether such text is generated,
/// or how many times, is unknowable here — so it is only safe to count
/// through it when it is balanced on its own: it leaves the block stack at
/// the depth (and the step kind) it found it.
struct MacroFrame {
    depth: usize,
    step: StepKind,
}

/// Tokens that can precede `do` / `end` when the keyword is being used as an
/// operand rather than as a statement — `x = end;`, `f(do)`. A real block
/// keyword never appears in any of these positions.
const EXPRESSION_PREFIXES: &[TokenType] = &[
    TokenType::ASSIGN,
    TokenType::LPAREN,
    TokenType::LBRACK,
    TokenType::COMMA,
    TokenType::PLUS,
    TokenType::MINUS,
    TokenType::STAR,
    TokenType::FSLASH,
    TokenType::STAR2,
    TokenType::EXCL,
    TokenType::EXCL2,
    TokenType::BPIPE,
    TokenType::BPIPE2,
];

impl Rule for UnbalancedDoBlock {
    fn id(&self) -> &'static str {
        ID
    }
    fn description(&self) -> &'static str {
        DESCRIPTION
    }
    fn severity(&self) -> Severity {
        // A hard SAS compile error ("ERROR 117-185: There were N unclosed DO
        // blocks."), and one SAS reports without a line number.
        Severity::Error
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        // `None` means a macro construct made the token-level count
        // untrustworthy; stay silent rather than risk a false positive.
        let mut findings = check_tokens(&ctx.tokens.default, ctx.path).unwrap_or_default();
        findings.sort_by_key(|f| (f.line, f.column));
        findings
    }
}

fn check_tokens(tokens: &[Token], path: &str) -> Option<Vec<Finding>> {
    let mut findings = Vec::new();
    let mut stack: Vec<(Opener, &Token)> = Vec::new();
    let mut macros: Vec<MacroFrame> = Vec::new();
    let mut step = StepKind::DataStep;

    for (i, t) in tokens.iter().enumerate() {
        let stmt_start = i == 0 || prev_default(tokens, i) == Some(TokenType::SEMI);

        match t.token_type {
            TokenType::KwmDo | TokenType::KwmMacro => {
                macros.push(MacroFrame {
                    depth: stack.len(),
                    step,
                });
                continue;
            }
            TokenType::KwmEnd | TokenType::KwmMend => {
                let frame = macros.pop()?;
                if frame.depth != stack.len() || frame.step != step {
                    return None;
                }
                continue;
            }
            _ => {}
        }

        // Step boundaries. Anything still open is unclosed, and an `end` in
        // the *next* step must not be credited against it. A data step ends
        // at `run;` or at the next `data` / `proc` statement; a run-group PROC
        // (SQL, DS2, TEMPLATE) runs on to `quit;` or the next step.
        if stmt_start {
            let next = tokens.get(i + 1).map(|n| n.token_type);
            let boundary = match t.token_type {
                TokenType::KwRun if next == Some(TokenType::SEMI) => Some(match step {
                    StepKind::Opaque => StepKind::DataStep,
                    other => other,
                }),
                TokenType::KwQuit if next == Some(TokenType::SEMI) => Some(StepKind::DataStep),
                TokenType::KwProc if next == Some(TokenType::Identifier) => {
                    Some(StepKind::for_proc(&tokens[i + 1].text))
                }
                // DS2 nests `data …; … enddata;` inside the PROC.
                TokenType::KwData if next != Some(TokenType::ASSIGN) && step != StepKind::Ds2 => {
                    Some(StepKind::DataStep)
                }
                _ => None,
            };
            if let Some(next_step) = boundary {
                drain(&mut findings, path, &mut stack);
                step = next_step;
                if below_macro_depth(&macros, &stack) {
                    return None;
                }
                continue;
            }
        }

        let opener = match (step, t.token_type) {
            (StepKind::DataStep | StepKind::Ds2, TokenType::KwDo) if opens_block(tokens, i) => {
                Some(Opener::Do)
            }
            (StepKind::DataStep | StepKind::Ds2, TokenType::KwSelect)
                if is_data_step_select(tokens, i) =>
            {
                Some(Opener::Select)
            }
            (StepKind::Ds2, TokenType::Identifier)
                if stmt_start && t.text.eq_ignore_ascii_case("method") =>
            {
                Some(Opener::Method)
            }
            (StepKind::Sql, TokenType::KwCase)
                if prev_default(tokens, i) != Some(TokenType::DOT) =>
            {
                Some(Opener::Case)
            }
            (StepKind::Template, TokenType::Identifier) if stmt_start => {
                if t.text.eq_ignore_ascii_case("define") {
                    Some(Opener::Define)
                } else if t.text.eq_ignore_ascii_case("edit") {
                    Some(Opener::Edit)
                } else {
                    None
                }
            }
            _ => None,
        };
        let closes = t.token_type == TokenType::KwEnd
            && match step {
                StepKind::DataStep | StepKind::Ds2 | StepKind::Template => closes_block(tokens, i),
                StepKind::Sql => prev_default(tokens, i) != Some(TokenType::DOT),
                StepKind::Opaque => false,
            };
        if opener.is_none() && !closes {
            continue;
        }

        // A block keyword emitted by a single-statement `%if … %then` /
        // `%else` is conditional text with no `%end` to balance against.
        if matches!(
            prev_default(tokens, i),
            Some(TokenType::KwmThen | TokenType::KwmElse)
        ) {
            return None;
        }

        if let Some(opener) = opener {
            stack.push((opener, t));
        } else if stack.pop().is_none() {
            // Inside a macro body a "stray" `end` may close a block its
            // caller opened.
            if !macros.is_empty() {
                return None;
            }
            findings.push(finding(path, t, step.stray_end_message().into()));
        } else if below_macro_depth(&macros, &stack) {
            return None;
        }
    }

    if !macros.is_empty() {
        return None;
    }
    // Fragments carry no `run;`, so EOF is the terminator.
    drain(&mut findings, path, &mut stack);
    Some(findings)
}

/// The innermost macro body closed a block it didn't open.
fn below_macro_depth(macros: &[MacroFrame], stack: &[(Opener, &Token)]) -> bool {
    macros.last().is_some_and(|m| stack.len() < m.depth)
}

/// Report every still-open block, in source order, and reset the stack.
fn drain(findings: &mut Vec<Finding>, path: &str, stack: &mut Vec<(Opener, &Token)>) {
    for (opener, tok) in stack.iter() {
        findings.push(finding(
            path,
            tok,
            format!("`{}` is never closed by a matching `end`", opener.keyword()),
        ));
    }
    stack.clear();
}

fn finding(path: &str, tok: &Token, message: String) -> Finding {
    Finding {
        path: path.to_string(),
        line: tok.start_line,
        column: tok.start_column + 1,
        rule: ID,
        message,
        severity: Severity::Error,
    }
}

fn prev_default(tokens: &[Token], i: usize) -> Option<TokenType> {
    i.checked_sub(1).map(|p| tokens[p].token_type)
}

fn followed_by_semi(tokens: &[Token], i: usize) -> bool {
    tokens.get(i + 1).map(|t| t.token_type) == Some(TokenType::SEMI)
}

/// `do` opens a block everywhere it appears as a keyword — plain `do;`,
/// iterative `do i = 1 to 10;`, `do while (…)`, `do until (…)`, `do over arr;`
/// — including after `then` / `else` / `when (…)` / `otherwise`, so position
/// can't be constrained further than "not an operand".
fn opens_block(tokens: &[Token], i: usize) -> bool {
    !matches!(prev_default(tokens, i), Some(ty) if EXPRESSION_PREFIXES.contains(&ty))
}

/// A block-closing `end` is always the whole statement: `end;`. Requiring the
/// `;` is what keeps `set b end = eof;` (data-step option) and `if end then …`
/// (a variable named `end`) from being read as block closes.
fn closes_block(tokens: &[Token], i: usize) -> bool {
    followed_by_semi(tokens, i) && opens_block(tokens, i)
}

/// Distinguish the data-step `select` — `select;` or `select (expr);`, closed
/// by `end;` — from the PROC SQL clause, which is followed by a column list.
fn is_data_step_select(tokens: &[Token], i: usize) -> bool {
    match tokens.get(i + 1).map(|t| t.token_type) {
        Some(TokenType::SEMI) => true,
        Some(TokenType::LPAREN) => {
            let mut depth = 0usize;
            for (j, t) in tokens.iter().enumerate().skip(i + 1) {
                match t.token_type {
                    TokenType::LPAREN => depth += 1,
                    TokenType::RPAREN => {
                        depth -= 1;
                        if depth == 0 {
                            return followed_by_semi(tokens, j);
                        }
                    }
                    // `select` clause ran to the end of a statement without
                    // closing its paren — not the data-step form.
                    TokenType::SEMI => return false,
                    _ => {}
                }
            }
            false
        }
        _ => false,
    }
}
