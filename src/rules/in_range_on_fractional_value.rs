//! `VAR in (N:M)` on a variable that can hold a fraction. A colon range in
//! an `in` list stands for the whole numbers `N, N+1, …, M`, so `28.4` is
//! not in `(5:70)`; authors almost always mean `N le VAR le M`, and SAS
//! compiles the test without a note.

use std::collections::HashMap;

use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::{Token, TokenStream};

use super::condition::{self, Lit, CONJUNCTION_OPS, DISJUNCTION_OPS};
use super::RuleMeta;

pub struct InRangeOnFractionalValue {
    autofix: bool,
}

const ID: &str = "in_range_on_fractional_value";
const DESCRIPTION: &str = "`VAR in (N:M)` on a variable that can hold a fraction — \
                            the range matches whole numbers only.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: true,
        default_factory: || Box::new(InRangeOnFractionalValue { autofix: false }),
        config_factory: |opts| {
            Ok(Box::new(InRangeOnFractionalValue {
                autofix: crate::config::opt_bool(opts, "autofix").unwrap_or(false),
            }))
        },
    }
}

impl Rule for InRangeOnFractionalValue {
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
        let tokens = &ctx.tokens.default;
        let lists = in_lists(tokens);
        let evidence = fractional_evidence(tokens, &lists);
        let mut findings = Vec::new();
        for list in &lists {
            let Some(ev) = evidence.get(&list.var.text.to_lowercase()) else {
                continue;
            };
            let written = list.source_text(ctx.source);
            for r in &list.ranges {
                findings.push(Finding {
                    path: ctx.path.to_string(),
                    line: r.first.start_line,
                    column: r.first.start_column + 1,
                    rule: ID,
                    message: format!(
                        "`{}` only matches whole numbers; {} is {} at line {}. \
                         Use `{} le {} le {}` to include fractions.",
                        written, list.var.text, ev.why, ev.line, r.lo, list.var.text, r.hi
                    ),
                    severity: Severity::Warning,
                });
            }
        }
        findings
    }

    /// Rewrites `VAR in (N:M)` to `N le VAR le M`. Only a list that is a
    /// single range: a mixed list (`in (0, 5:70)`) needs an `or`, and
    /// `not in` a `not (…)`, so those stay report-only.
    fn autofix(&self, source: &str) -> String {
        let stream = TokenStream::tokenize(source);
        let tokens = &stream.default;
        let lists = in_lists(tokens);
        let evidence = fractional_evidence(tokens, &lists);
        let mut edits: Vec<(usize, usize, String)> = lists
            .iter()
            .filter(|l| !l.negated && l.items == 1 && l.ranges.len() == 1)
            .filter(|l| evidence.contains_key(&l.var.text.to_lowercase()))
            .map(|l| {
                let r = &l.ranges[0];
                let test = format!("{} le {} le {}", r.lo, l.var.text, r.hi);
                let test = if bare_test_fits(tokens, l.var_idx, l.close_idx) {
                    test
                } else {
                    format!("({})", test)
                };
                let start = l.var.start_byte as usize;
                let end = tokens[l.close_idx].end_byte as usize;
                (start, end, test)
            })
            .collect();
        edits.sort_unstable_by_key(|e| std::cmp::Reverse(e.0));
        let mut out = source.to_string();
        for (start, end, test) in edits {
            out.replace_range(start..end, &test);
        }
        out
    }
}

/// `VAR [not] in ( … )` with at least one colon range.
struct InList<'a> {
    var: &'a Token,
    var_idx: usize,
    close_idx: usize,
    close_end: usize,
    negated: bool,
    /// Items in the list, ranges included.
    items: usize,
    ranges: Vec<ColonRange<'a>>,
    /// Non-integer literals in the list (`in (1.5, 2)`).
    fractions: Vec<&'a Token>,
}

impl InList<'_> {
    /// The test as written in `source` (`bmi in (5:70)`).
    fn source_text(&self, source: &str) -> String {
        let end = self.close_end;
        source
            .get(self.var.start_byte as usize..end)
            .unwrap_or_default()
            .to_string()
    }
}

struct ColonRange<'a> {
    /// The range's first token (the `-` of a negative bound).
    first: &'a Token,
    lo: String,
    hi: String,
}

/// Every `VAR in (…)` / `VAR not in (…)` in `tokens` that holds a colon
/// range or a fractional literal. Items may be separated by commas or
/// blanks, as SAS allows.
fn in_lists(tokens: &[Token]) -> Vec<InList<'_>> {
    let mut out = Vec::new();
    for k in 1..tokens.len() {
        if tokens[k].token_type != TokenType::KwIN
            || tokens.get(k + 1).map(|t| t.token_type) != Some(TokenType::LPAREN)
        {
            continue;
        }
        let negated = tokens[k - 1].token_type == TokenType::KwNOT;
        let var_idx = if negated { k.wrapping_sub(2) } else { k - 1 };
        let Some(var) = tokens
            .get(var_idx)
            .filter(|t| t.token_type == TokenType::Identifier)
        else {
            continue;
        };
        let Some(close_idx) = matching_paren(tokens, k + 1) else {
            continue;
        };
        let body = &tokens[k + 2..close_idx];
        let mut items = 0;
        let mut ranges = Vec::new();
        let mut fractions = Vec::new();
        let mut j = 0;
        while j < body.len() {
            if body[j].token_type == TokenType::COMMA {
                j += 1;
                continue;
            }
            items += 1;
            let Some((lit, used)) = condition::literal_at(body, j) else {
                j += 1;
                continue;
            };
            let hi = (body.get(j + used).map(|t| t.token_type) == Some(TokenType::COLON))
                .then(|| condition::literal_at(body, j + used + 1))
                .flatten();
            match (&lit, hi) {
                (Lit::Int(_), Some((Lit::Int(_), hi_used))) => {
                    let text = |s: &[Token]| s.iter().map(|t| t.text.as_str()).collect();
                    ranges.push(ColonRange {
                        first: &body[j],
                        lo: text(&body[j..j + used]),
                        hi: text(&body[j + used + 1..j + used + 1 + hi_used]),
                    });
                    j += used + 1 + hi_used;
                }
                _ => {
                    if matches!(lit, Lit::Float(_)) {
                        fractions.push(&body[j + used - 1]);
                    }
                    j += used;
                }
            }
        }
        if ranges.is_empty() && fractions.is_empty() {
            continue;
        }
        out.push(InList {
            var,
            var_idx,
            close_idx,
            close_end: tokens[close_idx].end_byte as usize,
            negated,
            items,
            ranges,
            fractions,
        });
    }
    out
}

/// Why a variable is thought to hold fractions, and where.
struct Evidence {
    line: u32,
    why: String,
}

/// Variables, lowercased, that can hold a non-integer: assigned an
/// expression that yields fractions, or else compared with a non-integer
/// literal. The first assignment wins; a comparison is the fallback.
fn fractional_evidence(tokens: &[Token], lists: &[InList]) -> HashMap<String, Evidence> {
    let mut out: HashMap<String, Evidence> = HashMap::new();
    for i in 0..tokens.len() {
        if tokens[i].token_type != TokenType::Identifier
            || tokens.get(i + 1).map(|t| t.token_type) != Some(TokenType::ASSIGN)
            || !starts_statement(tokens, i)
        {
            continue;
        }
        let end = tokens[i + 2..]
            .iter()
            .position(|t| t.token_type == TokenType::SEMI)
            .map_or(tokens.len(), |p| i + 2 + p);
        if let Some(why) = fraction_source(&tokens[i + 2..end]) {
            out.entry(tokens[i].text.to_lowercase())
                .or_insert(Evidence {
                    line: tokens[i].start_line,
                    why,
                });
        }
    }
    let mut compared = |var: &Token, lit: &Token| {
        out.entry(var.text.to_lowercase()).or_insert(Evidence {
            line: lit.start_line,
            why: format!("compared with {}", lit.text),
        });
    };
    for i in 0..tokens.len() {
        // `VAR op 18.5`
        if tokens[i].token_type == TokenType::Identifier
            && tokens
                .get(i + 1)
                .is_some_and(|t| is_comparison(t.token_type))
            && !(tokens[i + 1].token_type == TokenType::ASSIGN && starts_statement(tokens, i))
            && !i
                .checked_sub(1)
                .is_some_and(|p| is_arithmetic(tokens[p].token_type))
        {
            if let Some((Lit::Float(_), used)) = condition::literal_at(tokens, i + 2) {
                if !tokens
                    .get(i + 2 + used)
                    .is_some_and(|t| is_arithmetic(t.token_type))
                {
                    compared(&tokens[i], &tokens[i + 1 + used]);
                }
            }
        }
        // `18.5 op VAR`
        if let Some((Lit::Float(_), used)) = condition::literal_at(tokens, i) {
            let op = tokens.get(i + used);
            let var = tokens.get(i + used + 1);
            if op.is_some_and(|t| is_comparison(t.token_type) && t.token_type != TokenType::ASSIGN)
                && var.is_some_and(|t| t.token_type == TokenType::Identifier)
                && !i.checked_sub(1).is_some_and(|p| {
                    is_arithmetic(tokens[p].token_type) || is_operand(tokens[p].token_type)
                })
                && !tokens
                    .get(i + used + 2)
                    .is_some_and(|t| is_arithmetic(t.token_type))
            {
                compared(&tokens[i + used + 1], &tokens[i + used - 1]);
            }
        }
    }
    for list in lists {
        if let Some(lit) = list.fractions.first() {
            compared(list.var, lit);
        }
    }
    out
}

/// What makes an assignment's right-hand side fractional, if anything:
/// a division, a non-integer literal, a negative or fractional power, or a
/// function that returns fractions. Inside `floor`, `int`, `round(x)` and
/// other whole-number functions none of that counts.
fn fraction_source(rhs: &[Token]) -> Option<String> {
    let mut k = 0;
    while k < rhs.len() {
        let t = &rhs[k];
        if is_name(t) && rhs.get(k + 1).map(|n| n.token_type) == Some(TokenType::LPAREN) {
            let name = t.text.to_lowercase();
            let close = matching_paren(rhs, k + 1).unwrap_or(rhs.len());
            if returns_whole(&name, &rhs[k + 2..close]) {
                k = close + 1;
                continue;
            }
            if FRACTIONAL_FUNCTIONS.contains(&name.as_str()) {
                return Some(format!("assigned the result of `{}`", name));
            }
            k += 2;
            continue;
        }
        match t.token_type {
            TokenType::FSLASH => return Some("assigned a quotient".to_string()),
            TokenType::FloatLiteral | TokenType::FloatExponentLiteral => {
                if let Some((Lit::Float(_), _)) = condition::literal_at(rhs, k) {
                    return Some(format!("assigned an expression with {}", t.text));
                }
            }
            TokenType::STAR2 => {
                if let Some((Lit::Float(_) | Lit::Int(i64::MIN..=-1), _)) =
                    condition::literal_at(rhs, k + 1)
                {
                    return Some("assigned a fractional power".to_string());
                }
            }
            _ => {}
        }
        k += 1;
    }
    None
}

/// Functions whose result can have a fraction whatever their arguments.
const FRACTIONAL_FUNCTIONS: &[&str] = &[
    "arcos", "arsin", "atan", "atan2", "beta", "cdf", "cos", "cosh", "cv", "divide", "exp",
    "gamma", "geomean", "harmean", "lgamma", "log", "log10", "log1px", "log2", "logcdf", "logpdf",
    "mean", "median", "normal", "pctl", "pdf", "probnorm", "quantile", "rand", "ranexp", "rannor",
    "ranuni", "sin", "sinh", "sqrt", "std", "stderr", "tan", "tanh", "uniform", "var",
];

/// Functions whose result is a whole number whatever their arguments.
const WHOLE_FUNCTIONS: &[&str] = &[
    "ceil", "ceilz", "cmiss", "count", "countc", "countw", "day", "find", "findc", "findw",
    "floor", "floorz", "hour", "index", "indexc", "indexw", "int", "intck", "intz", "length",
    "lengthc", "lengthm", "lengthn", "minute", "missing", "month", "n", "nmiss", "qtr", "sign",
    "verify", "weekday", "year",
];

/// Does `name(args)` return a whole number? `round` does with one argument
/// or a whole rounding unit (`round(x, 1)`), not with `round(x, 0.1)`.
fn returns_whole(name: &str, args: &[Token]) -> bool {
    if WHOLE_FUNCTIONS.contains(&name) {
        return true;
    }
    if !matches!(name, "round" | "roundz") {
        return false;
    }
    let mut depth = 0usize;
    let comma = args.iter().position(|t| {
        match t.token_type {
            TokenType::LPAREN => depth += 1,
            TokenType::RPAREN => depth = depth.saturating_sub(1),
            _ => {}
        }
        depth == 0 && t.token_type == TokenType::COMMA
    });
    match comma {
        None => true,
        Some(c) => matches!(
            condition::literal_at(args, c + 1),
            Some((Lit::Int(n), used)) if n >= 1 && c + 1 + used == args.len()
        ),
    }
}

/// An identifier at the start of a statement: the target of an
/// assignment rather than an operand of a comparison. A `do` loop index
/// is not counted (`do i = 1 to n/2`).
fn starts_statement(tokens: &[Token], i: usize) -> bool {
    i == 0
        || matches!(
            tokens[i - 1].token_type,
            TokenType::SEMI | TokenType::KwThen | TokenType::KwElse
        )
}

/// A function name: an identifier, or a keyword the lexer gives its own
/// type (`index`, `missing`).
fn is_name(t: &Token) -> bool {
    t.text
        .starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && t.text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_comparison(ty: TokenType) -> bool {
    matches!(
        ty,
        TokenType::LT
            | TokenType::LE
            | TokenType::GT
            | TokenType::GE
            | TokenType::NE
            | TokenType::ASSIGN
            | TokenType::KwLT
            | TokenType::KwLE
            | TokenType::KwGT
            | TokenType::KwGE
            | TokenType::KwNE
            | TokenType::KwEQ
    )
}

/// A token that ends an operand, so a `-` after it subtracts.
fn is_operand(ty: TokenType) -> bool {
    matches!(
        ty,
        TokenType::Identifier
            | TokenType::RPAREN
            | TokenType::IntegerLiteral
            | TokenType::FloatLiteral
            | TokenType::FloatExponentLiteral
    )
}

fn is_arithmetic(ty: TokenType) -> bool {
    matches!(
        ty,
        TokenType::PLUS | TokenType::MINUS | TokenType::STAR | TokenType::FSLASH | TokenType::STAR2
    )
}

/// Can `N le VAR le M` replace the test at `tokens[var_idx..=close_idx]`
/// without parentheses? Only between operators that bind looser than a
/// comparison (`and`, `or`), a statement's `if` / `then` / `;`, a
/// bracket, or an assignment's `=`.
fn bare_test_fits(tokens: &[Token], var_idx: usize, close_idx: usize) -> bool {
    let loose = |ty: TokenType| CONJUNCTION_OPS.contains(&ty) || DISJUNCTION_OPS.contains(&ty);
    let before = match var_idx.checked_sub(1).map(|p| tokens[p].token_type) {
        None => true,
        Some(TokenType::ASSIGN) => var_idx >= 2 && starts_statement(tokens, var_idx - 2),
        Some(ty) => {
            loose(ty)
                || matches!(
                    ty,
                    TokenType::KwIf
                        | TokenType::KwWhere
                        | TokenType::KwWhen
                        | TokenType::LPAREN
                        | TokenType::COMMA
                )
        }
    };
    let after = tokens.get(close_idx + 1).is_none_or(|t| {
        loose(t.token_type)
            || matches!(
                t.token_type,
                TokenType::KwThen | TokenType::SEMI | TokenType::RPAREN | TokenType::COMMA
            )
    });
    before && after
}

/// The index of the `)` matching the `(` at `open`.
fn matching_paren(tokens: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (j, t) in tokens.iter().enumerate().skip(open) {
        match t.token_type {
            TokenType::LPAREN => depth += 1,
            TokenType::RPAREN => {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
            _ => {}
        }
    }
    None
}
