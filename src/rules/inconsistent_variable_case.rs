use std::collections::{HashMap, HashSet};

use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::{Token, TokenStream};

use super::sas::{FORMAT_STATEMENTS, RUN_GROUP_PROCS};
use super::RuleMeta;

pub struct InconsistentVariableCase {
    autofix: bool,
}

const ID: &str = "inconsistent_variable_case";
const DESCRIPTION: &str = "Variable identifiers must use one consistent letter case across the \
     file; mixing `myVar` and `MyVar` is sloppy.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: true,
        default_factory: || Box::new(InconsistentVariableCase { autofix: false }),
        config_factory: |opts| {
            Ok(Box::new(InconsistentVariableCase {
                autofix: crate::config::opt_bool(opts, "autofix").unwrap_or(false),
            }))
        },
    }
}

impl Rule for InconsistentVariableCase {
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
        let mut findings = Vec::new();
        for (token, canonical) in inconsistent_uses(&ctx.tokens.default) {
            findings.push(Finding {
                path: ctx.path.to_string(),
                line: token.start_line,
                column: token.start_column + 1,
                rule: ID,
                message: format!(
                    "variable `{}` is spelled `{}` elsewhere in this file — \
                     pick one case and stick with it.",
                    token.text, canonical
                ),
                severity: Severity::Warning,
            });
        }
        findings
    }

    fn autofix(&self, source: &str) -> String {
        if source.is_empty() {
            return source.to_string();
        }
        let stream = TokenStream::tokenize(source);
        let mut edits: Vec<(u32, u32, String)> = inconsistent_uses(&stream.default)
            .into_iter()
            .map(|(t, canon)| (t.start_byte, t.end_byte, canon))
            .collect();
        // Apply right-to-left so earlier byte offsets remain valid.
        edits.sort_by_key(|e| std::cmp::Reverse(e.0));

        let mut out = source.to_string();
        for (start, end, repl) in edits {
            let start = start as usize;
            let end = end as usize;
            if start <= out.len() && end <= out.len() && start <= end {
                out.replace_range(start..end, &repl);
            }
        }
        out
    }
}

fn inconsistent_uses(tokens: &[Token]) -> Vec<(Token, String)> {
    let mut groups: HashMap<String, Vec<Token>> = HashMap::new();
    for i in variable_uses(tokens) {
        let t = &tokens[i];
        groups
            .entry(t.text.to_lowercase())
            .or_default()
            .push(t.clone());
    }

    let mut out = Vec::new();
    for (_key, uses) in groups {
        if uses.len() < 2 {
            continue;
        }
        let mut counts: HashMap<String, usize> = HashMap::new();
        for u in &uses {
            *counts.entry(u.text.clone()).or_default() += 1;
        }
        if counts.len() <= 1 {
            continue;
        }
        let canonical = canonical_form(&counts, &uses);
        for u in &uses {
            if u.text != canonical {
                out.push((u.clone(), canonical.clone()));
            }
        }
    }
    out
}

fn canonical_form(counts: &HashMap<String, usize>, uses: &[Token]) -> String {
    let max_count = counts.values().copied().max().unwrap_or(0);
    let winners: Vec<&String> = counts
        .iter()
        .filter_map(|(k, v)| if *v == max_count { Some(k) } else { None })
        .collect();
    if winners.len() == 1 {
        return winners[0].clone();
    }
    for u in uses {
        if winners.contains(&&u.text) {
            return u.text.clone();
        }
    }
    winners[0].clone()
}

/// Leading words of data-step statements whose operands are variable lists
/// or expressions. Any other statement (`set`, `output`, `infile`, `goto`,
/// `options`, `title`, …) names datasets, files, labels or options instead,
/// and is skipped.
const VARIABLE_STATEMENTS: &[&str] = &[
    "array", "attrib", "by", "call", "do", "drop", "format", "informat", "input", "keep", "label",
    "length", "put", "putlog", "rename", "retain", "select", "where",
];

/// Words the lexer leaves as identifiers inside variable statements without
/// them naming a variable: `sum(of a b)`, `do over arr`, `array t{3}
/// _temporary_`.
const NON_VARIABLE_WORDS: &[&str] = &["of", "over", "_temporary_"];

/// Indices of every identifier that sits in a data-step variable position,
/// in source order. PROC step bodies are skipped wholesale: their option
/// names and arguments live in other namespaces.
fn variable_uses(tokens: &[Token]) -> Vec<usize> {
    let statements = split_statements(tokens);
    let arrays: HashSet<String> = statements
        .iter()
        .filter(|&&(s, e)| tokens[s].token_type == TokenType::KwArray && s + 1 < e)
        .filter(|&&(s, _)| tokens[s + 1].token_type == TokenType::Identifier)
        .map(|&(s, _)| tokens[s + 1].text.to_ascii_lowercase())
        .collect();

    let mut out = Vec::new();
    let mut proc: Option<String> = None;
    for (s, e) in statements {
        let next = tokens.get(s + 1).filter(|_| s + 1 < e);
        match tokens[s].token_type {
            TokenType::KwProc => {
                proc = Some(
                    next.map(|n| n.text.to_ascii_lowercase())
                        .unwrap_or_default(),
                );
                continue;
            }
            // DS2 nests `data …; … enddata;` inside the PROC.
            TokenType::KwData
                if next.map(|n| n.token_type) != Some(TokenType::ASSIGN)
                    && proc.as_deref() != Some("ds2") =>
            {
                proc = None;
                continue;
            }
            TokenType::KwRun if e == s + 1 => {
                if !proc
                    .as_deref()
                    .is_some_and(|p| RUN_GROUP_PROCS.contains(&p))
                {
                    proc = None;
                }
                continue;
            }
            TokenType::KwQuit if e == s + 1 => {
                proc = None;
                continue;
            }
            _ => {}
        }
        // Sources commonly open with a `run;`-less `proc format; value …;`
        // block and continue straight into the data-step body; PROC FORMAT
        // has a closed statement vocabulary, so anything else ends it.
        if proc.as_deref() == Some("format")
            && !FORMAT_STATEMENTS.contains(&tokens[s].text.to_ascii_lowercase().as_str())
        {
            proc = None;
        }
        if proc.is_none() {
            statement_uses(tokens, s, e, &arrays, &mut out);
        }
    }
    out
}

/// `[start, end)` token ranges of each statement, excluding its `;`.
fn split_statements(tokens: &[Token]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, t) in tokens.iter().enumerate() {
        if t.token_type == TokenType::SEMI {
            if start < i {
                out.push((start, i));
            }
            start = i + 1;
        }
    }
    if start < tokens.len() {
        out.push((start, tokens.len()));
    }
    out
}

/// Collect the variable uses of the statement in `tokens[s..e]`. Walks
/// through the statement prefixes that introduce another statement —
/// labels, `if … then`, `else`, `when (…)`, `otherwise`, `%if … %then` —
/// and classifies whatever statement follows.
fn statement_uses(
    tokens: &[Token],
    mut s: usize,
    e: usize,
    arrays: &HashSet<String>,
    out: &mut Vec<usize>,
) {
    while s < e {
        let t = &tokens[s];
        let next = tokens
            .get(s + 1)
            .filter(|_| s + 1 < e)
            .map(|n| n.token_type);

        if t.text.starts_with('%') {
            // Macro statement: only the statement generated by `%then` /
            // `%else` is data-step code.
            match t.token_type {
                TokenType::KwmIf => match find(tokens, s + 1, e, TokenType::KwmThen) {
                    Some(then) => s = then + 1,
                    None => return,
                },
                TokenType::KwmElse => s += 1,
                _ => return,
            }
            continue;
        }
        if t.token_type == TokenType::Identifier && next == Some(TokenType::COLON) {
            s += 2; // statement label
            continue;
        }
        // `otherwise` is not a keyword token.
        if t.token_type == TokenType::Identifier
            && t.text.eq_ignore_ascii_case("otherwise")
            && next != Some(TokenType::ASSIGN)
        {
            s += 1;
            continue;
        }
        match t.token_type {
            TokenType::KwIf => match find(tokens, s + 1, e, TokenType::KwThen) {
                Some(then) => {
                    operand_uses(tokens, s + 1, then, arrays, out);
                    s = then + 1;
                    continue;
                }
                // Subsetting `if`.
                None => return operand_uses(tokens, s + 1, e, arrays, out),
            },
            TokenType::KwElse => {
                s += 1;
                continue;
            }
            TokenType::KwWhen if next == Some(TokenType::LPAREN) => {
                let close = matching_paren(tokens, s + 1, e);
                operand_uses(tokens, s + 1, close, arrays, out);
                s = close;
                continue;
            }
            _ => {}
        }
        // Assignment (`x = …`, `arr{i} = …`, `substr(x, 1, 1) = …`) or sum
        // statement (`x + 1`). Checked before the keyword table: a variable
        // may well be called `file` or `set`.
        let assignment = next == Some(TokenType::ASSIGN)
            || (t.token_type == TokenType::Identifier
                && matches!(
                    next,
                    Some(
                        TokenType::PLUS | TokenType::LPAREN | TokenType::LCURLY | TokenType::LBRACK
                    )
                ));
        if assignment {
            return operand_uses(tokens, s, e, arrays, out);
        }
        if VARIABLE_STATEMENTS.contains(&t.text.to_ascii_lowercase().as_str()) {
            return operand_uses(tokens, s + 1, e, arrays, out);
        }
        return;
    }
}

/// Identifiers in `tokens[lo..hi]` that name variables: not a function or
/// CALL-routine name (followed by `(` without being a declared array), not
/// part of a dotted name (`lib.ds`, `h.find`, a format `fmt.`).
fn operand_uses(
    tokens: &[Token],
    lo: usize,
    hi: usize,
    arrays: &HashSet<String>,
    out: &mut Vec<usize>,
) {
    for i in lo..hi {
        let t = &tokens[i];
        if t.token_type != TokenType::Identifier {
            continue;
        }
        let lower = t.text.to_ascii_lowercase();
        if NON_VARIABLE_WORDS.contains(&lower.as_str()) {
            continue;
        }
        let nxt = tokens.get(i + 1);
        let prev = i.checked_sub(1).map(|p| &tokens[p]);
        if nxt.is_some_and(|n| n.token_type == TokenType::DOT && n.start_byte == t.end_byte)
            || prev.is_some_and(|p| p.token_type == TokenType::DOT && p.end_byte == t.start_byte)
        {
            continue;
        }
        if nxt.is_some_and(|n| n.token_type == TokenType::LPAREN) && !arrays.contains(&lower) {
            continue;
        }
        out.push(i);
    }
}

/// First `ty` token in `tokens[lo..hi]` outside parentheses.
fn find(tokens: &[Token], lo: usize, hi: usize, ty: TokenType) -> Option<usize> {
    let mut depth = 0usize;
    for (i, t) in tokens.iter().enumerate().take(hi).skip(lo) {
        match t.token_type {
            TokenType::LPAREN => depth += 1,
            TokenType::RPAREN => depth = depth.saturating_sub(1),
            other if other == ty && depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

/// Index just past the `)` matching the `(` at `open`, or `hi`.
fn matching_paren(tokens: &[Token], open: usize, hi: usize) -> usize {
    let mut depth = 0usize;
    for (i, t) in tokens.iter().enumerate().take(hi).skip(open) {
        match t.token_type {
            TokenType::LPAREN => depth += 1,
            TokenType::RPAREN => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
    }
    hi
}
