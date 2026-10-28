use std::collections::{HashMap, HashSet};

use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::Token;

use super::condition::{self, CmpOp, Lit};
use super::RuleMeta;

pub struct UnreachableInnerBranchValue;

const ID: &str = "unreachable_inner_branch_value";
const DESCRIPTION: &str = "Inner branch tests a variable against a value that the \
                            enclosing `if VAR in (...) then do;` guard excludes — the \
                            branch can never fire for that value.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: false,
        default_factory: || Box::new(UnreachableInnerBranchValue),
        config_factory: |_| Ok(Box::new(UnreachableInnerBranchValue)),
    }
}

#[derive(Debug)]
struct GuardFrame {
    var: String,
    allowed: HashSet<Lit>,
    depth: i32,
    line: u32,
    /// The guard's test as written (`x = 4`, `e in (0,1,2)`), for messages.
    text: String,
}

impl Rule for UnreachableInnerBranchValue {
    fn id(&self) -> &'static str {
        ID
    }
    fn description(&self) -> &'static str {
        DESCRIPTION
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        let tokens = &ctx.tokens.default;
        let mut findings = Vec::new();
        let mut stack: Vec<GuardFrame> = Vec::new();
        let mut do_depth: i32 = 0;
        let arrays = array_members(tokens);
        // Index of the next statement's first token.
        let mut stmt_start = 0;
        let mut i = 0;

        while i < tokens.len() {
            let tok = &tokens[i];
            // A guard says what a variable held when the block was entered.
            // Once the block writes the variable, the guard no longer
            // constrains it.
            if i == stmt_start {
                match writes(tokens, i, &arrays) {
                    Some(Writes::Vars(vars)) => stack.retain(|f| !vars.contains(&f.var)),
                    Some(Writes::Any) => stack.clear(),
                    None => {}
                }
            }
            match tok.token_type {
                TokenType::SEMI | TokenType::KwThen | TokenType::KwElse => stmt_start = i + 1,
                TokenType::Identifier if tok.text.eq_ignore_ascii_case("otherwise") => {
                    stmt_start = i + 1
                }
                TokenType::KwWhen
                    if tokens.get(i + 1).map(|t| t.token_type) == Some(TokenType::LPAREN) =>
                {
                    stmt_start = past_parens(tokens, i + 1)
                }
                _ => {}
            }
            match tok.token_type {
                TokenType::KwIf => {
                    let (consumed, frames, inner) =
                        analyze_if(tokens, i, do_depth, &stack, ctx.path, ctx.source);
                    findings.extend(inner);
                    // Frames take effect inside the `do;` that follows the
                    // `then`; the `do` itself is left for the arm below.
                    stack.extend(frames);
                    i += consumed;
                    continue;
                }
                TokenType::KwDo => {
                    do_depth += 1;
                    i += 1;
                    continue;
                }
                TokenType::KwSelect if opens_select_block(tokens, i) => {
                    do_depth += 1;
                    i += 1;
                    continue;
                }
                // A block-closing `end` is the whole statement, `end;`:
                // `infile … end = eof` and SQL `case … end as x` are not.
                TokenType::KwEnd
                    if tokens.get(i + 1).map(|t| t.token_type) == Some(TokenType::SEMI) =>
                {
                    if do_depth > 0 {
                        do_depth -= 1;
                    }
                    while let Some(last) = stack.last() {
                        if last.depth > do_depth {
                            stack.pop();
                        } else {
                            break;
                        }
                    }
                    i += 1;
                    continue;
                }
                _ => {
                    i += 1;
                }
            }
        }
        findings
    }
}

/// Variables a statement may write.
enum Writes {
    Vars(Vec<String>),
    /// Unknown — any variable.
    Any,
}

/// The variables the statement starting at `i` writes, if any: the target
/// of an assignment or sum statement, an iterative `do` index, the
/// variables of an `input` statement or a `call` routine's arguments, the
/// members of an array whose element is assigned. `set` / `merge` /
/// `update` / `modify` may overwrite anything.
fn writes(
    tokens: &[Token],
    i: usize,
    arrays: &HashMap<String, Option<Vec<String>>>,
) -> Option<Writes> {
    let ty = |j: usize| tokens.get(j).map(|t| t.token_type);
    let t = &tokens[i];
    let name = |j: usize| tokens[j].text.to_lowercase();
    let idents_until_semi = |from: usize| {
        tokens[from.min(tokens.len())..]
            .iter()
            .take_while(|t| t.token_type != TokenType::SEMI)
            .filter(|t| t.token_type == TokenType::Identifier)
            .map(|t| t.text.to_lowercase())
            .collect()
    };
    match t.token_type {
        TokenType::Identifier => match ty(i + 1)? {
            TokenType::ASSIGN | TokenType::PLUS => Some(Writes::Vars(vec![name(i)])),
            TokenType::LCURLY | TokenType::LPAREN | TokenType::LBRACK
                if ty(past_parens(tokens, i + 1)) == Some(TokenType::ASSIGN) =>
            {
                match arrays.get(&name(i)) {
                    Some(Some(members)) => Some(Writes::Vars(members.clone())),
                    _ => Some(Writes::Any),
                }
            }
            _ if t.text.eq_ignore_ascii_case("modify") => Some(Writes::Any),
            _ => None,
        },
        TokenType::KwDo
            if ty(i + 1) == Some(TokenType::Identifier) && ty(i + 2) == Some(TokenType::ASSIGN) =>
        {
            Some(Writes::Vars(vec![name(i + 1)]))
        }
        TokenType::KwInput => Some(Writes::Vars(idents_until_semi(i + 1))),
        // Skip the routine name; every name among the arguments may be set.
        TokenType::KwCall => Some(Writes::Vars(idents_until_semi(i + 2))),
        TokenType::KwSet | TokenType::KwMerge | TokenType::KwUpdate => Some(Writes::Any),
        _ => None,
    }
}

/// Every `array NAME{…} members;` in the file, by lowercased name. `None`
/// when the members can't be listed by name (implicit `NAME1`-`NAMEn`,
/// `_numeric_`, `x1-x9` ranges); a `_temporary_` array has none.
fn array_members(tokens: &[Token]) -> HashMap<String, Option<Vec<String>>> {
    let mut arrays = HashMap::new();
    for (i, t) in tokens.iter().enumerate() {
        if t.token_type != TokenType::KwArray {
            continue;
        }
        let Some(name) = tokens
            .get(i + 1)
            .filter(|t| t.token_type == TokenType::Identifier)
        else {
            continue;
        };
        let mut members = Some(Vec::new());
        let mut temporary = false;
        for m in tokens[past_parens(tokens, i + 2)..]
            .iter()
            .take_while(|t| !matches!(t.token_type, TokenType::SEMI | TokenType::LPAREN))
        {
            match m.token_type {
                TokenType::Identifier if m.text.eq_ignore_ascii_case("_temporary_") => {
                    temporary = true
                }
                TokenType::Identifier if m.text.starts_with('_') => members = None,
                TokenType::Identifier => {
                    if let Some(v) = members.as_mut() {
                        v.push(m.text.to_lowercase());
                    }
                }
                TokenType::MINUS | TokenType::COLON => members = None,
                _ => {}
            }
        }
        if members.as_ref().is_some_and(|v| v.is_empty()) && !temporary {
            members = None;
        }
        arrays.insert(name.text.to_lowercase(), members);
    }
    arrays
}

/// `i` is an opening bracket; return the index just past its match (or the
/// `;` / EOF that arrives first). Any other token: `i` itself.
fn past_parens(tokens: &[Token], i: usize) -> usize {
    let opens = |ty| {
        matches!(
            ty,
            TokenType::LPAREN | TokenType::LCURLY | TokenType::LBRACK
        )
    };
    if !tokens.get(i).is_some_and(|t| opens(t.token_type)) {
        return i;
    }
    let mut depth = 0usize;
    for (j, t) in tokens.iter().enumerate().skip(i) {
        match t.token_type {
            ty if opens(ty) => depth += 1,
            TokenType::RPAREN | TokenType::RCURLY | TokenType::RBRACK => {
                depth -= 1;
                if depth == 0 {
                    return j + 1;
                }
            }
            TokenType::SEMI => return j,
            _ => {}
        }
    }
    tokens.len()
}

/// Is the `select` at `i` a data-step `select;` / `select (expr);` block,
/// closed by `end;`? A PROC SQL `select a, …` / `select (a) from …` query
/// is not.
fn opens_select_block(tokens: &[Token], i: usize) -> bool {
    let after = past_parens(tokens, i + 1);
    tokens.get(after).map(|t| t.token_type) == Some(TokenType::SEMI)
}

/// Analyse the `if` at `i`. Returns how many tokens to skip (the condition,
/// up to but not including `then`), the guard frames the statement opens if
/// it is a `then do;` block, and the findings for values the enclosing
/// guards exclude.
fn analyze_if(
    tokens: &[Token],
    i: usize,
    do_depth: i32,
    stack: &[GuardFrame],
    path: &str,
    source: &str,
) -> (usize, Vec<GuardFrame>, Vec<Finding>) {
    let Some(then_idx) = find_then(tokens, i + 1) else {
        return (1, vec![], vec![]);
    };
    let cond = &tokens[i + 1..then_idx];
    let is_guard = tokens.get(then_idx + 1).map(|t| t.token_type) == Some(TokenType::KwDo)
        && tokens.get(then_idx + 2).map(|t| t.token_type) == Some(TokenType::SEMI);

    let mut findings = Vec::new();
    let mut frames = Vec::new();
    for cmp in conjunct_comparisons(cond) {
        let var = cmp.var_key();
        if let Some(frame) = stack.iter().rev().find(|f| f.var == var) {
            let dead: Vec<&condition::Value> = cmp
                .values
                .iter()
                .filter(|v| !frame.allowed.contains(&v.lit))
                .collect();
            // Every value excluded: the test can never be true, so the
            // whole branch is dead. Some excluded: the branch still fires
            // for the live values, only the dead ones are noise.
            let verdict = if dead.len() == cmp.values.len() {
                "this branch is unreachable."
            } else {
                "this branch can never fire for that value."
            };
            for val in dead {
                findings.push(Finding {
                    path: path.to_string(),
                    line: val.tok.start_line,
                    column: val.tok.start_column + 1,
                    rule: ID,
                    message: format!(
                        "value {} for {} is excluded by the enclosing \
                         `if {}` guard at line {}; {}",
                        val.display, cmp.var.text, frame.text, frame.line, verdict
                    ),
                    severity: Severity::Warning,
                });
            }
        }
        if is_guard {
            frames.push(GuardFrame {
                var,
                allowed: cmp.values.iter().map(|v| v.lit.clone()).collect(),
                depth: do_depth + 1,
                line: tokens[i].start_line,
                text: cmp.source_text(source),
            });
        }
    }
    (then_idx - i, frames, findings)
}

/// Index of the `then` closing the condition that starts at `i`, or `None`
/// if a `;` (or EOF) comes first.
fn find_then(tokens: &[Token], i: usize) -> Option<usize> {
    tokens[i..]
        .iter()
        .position(|t| matches!(t.token_type, TokenType::KwThen | TokenType::SEMI))
        .map(|p| i + p)
        .filter(|&j| tokens[j].token_type == TokenType::KwThen)
}

/// The `VAR in (…)` / `VAR = lit` tests in a condition, one per top-level
/// `and` conjunct. Conjuncts of any other shape (`not`, `ne`, ranges,
/// function calls, …) are skipped; a top-level disjunction yields nothing:
/// a dead value in one disjunct doesn't make the branch unreachable, and as
/// a guard it doesn't restrict anything.
fn conjunct_comparisons(cond: &[Token]) -> Vec<condition::Comparison<'_>> {
    condition::split_conjuncts(cond)
        .unwrap_or_default()
        .into_iter()
        .filter_map(condition::comparison)
        .filter(|cmp| cmp.op == CmpOp::In)
        .collect()
}
