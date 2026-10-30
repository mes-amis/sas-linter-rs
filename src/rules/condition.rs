//! Shared reading of `if` conditions for the rules that reason about the
//! values a test admits (`unreachable_else_if_branch`,
//! `unreachable_inner_branch_value`, `variable_value_out_of_known_range`).
//! One parser, so the rules can't disagree about what `'A'`, `-1` or `1e3`
//! mean, or about which operators join a condition's tests.

use sas_lexer::TokenType;

use crate::token::Token;

/// A literal operand, normalised to the value SAS compares.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Lit {
    Int(i64),
    /// Non-integral float, by bit pattern (so it can sit in a set).
    Float(u64),
    /// String literal contents: quotes stripped, doubled quotes collapsed,
    /// trailing blanks dropped (SAS pads the shorter side of a character
    /// comparison with blanks, so `'A '` and `"A"` are equal). Case is
    /// significant.
    Str(String),
    /// A missing value: `.` (`None`), or a special missing `.a`-`.z` /
    /// `._`, lowercased. Each is distinct: `.a = .` is false.
    Missing(Option<char>),
}

impl Lit {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Lit::Int(n) => Some(*n as f64),
            Lit::Float(bits) => Some(f64::from_bits(*bits)),
            Lit::Str(_) | Lit::Missing(_) => None,
        }
    }
}

fn num_lit(f: f64) -> Lit {
    if f.is_finite() && f == f.trunc() && f.abs() < i64::MAX as f64 {
        Lit::Int(f as i64)
    } else {
        Lit::Float(f.to_bits())
    }
}

/// A literal at `k`, allowing a leading unary minus. Returns the value and
/// the number of tokens consumed.
pub fn literal_at(toks: &[Token], k: usize) -> Option<(Lit, usize)> {
    let t = toks.get(k)?;
    match t.token_type {
        TokenType::MINUS => {
            let (lit, used) = literal_at(toks, k + 1)?;
            let neg = match lit {
                Lit::Int(n) => Lit::Int(-n),
                Lit::Float(bits) => num_lit(-f64::from_bits(bits)),
                Lit::Str(_) | Lit::Missing(_) => return None,
            };
            Some((neg, used + 1))
        }
        TokenType::IntegerLiteral | TokenType::FloatLiteral | TokenType::FloatExponentLiteral => {
            let f: f64 = t.text.parse().ok()?;
            f.is_finite().then(|| (num_lit(f), 1))
        }
        TokenType::StringLiteral => Some((Lit::Str(string_value(&t.text)), 1)),
        // `.a` lexes as DOT + a one-letter name with no space between.
        TokenType::DOT => match toks.get(k + 1) {
            Some(n)
                if n.token_type == TokenType::Identifier
                    && n.start_byte == t.end_byte
                    && n.text.len() == 1
                    && n.text.chars().all(|c| c.is_ascii_alphabetic() || c == '_') =>
            {
                let c = n.text.chars().next().map(|c| c.to_ascii_lowercase());
                Some((Lit::Missing(c), 2))
            }
            _ => Some((Lit::Missing(None), 1)),
        },
        _ => None,
    }
}

/// The value of a quoted string literal's text.
fn string_value(text: &str) -> String {
    let quote = match text.chars().next() {
        Some(q @ ('\'' | '"')) if text.len() >= 2 && text.ends_with(q) => q,
        _ => return text.trim_end_matches(' ').to_string(),
    };
    let inner = &text[1..text.len() - 1];
    let doubled: String = [quote, quote].iter().collect();
    inner
        .replace(&doubled, &quote.to_string())
        .trim_end_matches(' ')
        .to_string()
}

/// Operators that join two tests with OR: `or`, `|`, `!` and the
/// mainframe broken bar `¦`. (Doubled, `||` / `!!` / `¦¦` concatenate.)
pub const DISJUNCTION_OPS: &[TokenType] = &[
    TokenType::KwOR,
    TokenType::PIPE,
    TokenType::EXCL,
    TokenType::BPIPE,
];

/// Operators that join two tests with AND: `and`, `&`.
pub const CONJUNCTION_OPS: &[TokenType] = &[TokenType::KwAND, TokenType::AMP];

/// Peel `( … )` pairs that enclose the entire slice.
pub fn strip_outer_parens(mut toks: &[Token]) -> &[Token] {
    loop {
        let n = toks.len();
        if n < 2
            || toks[0].token_type != TokenType::LPAREN
            || toks[n - 1].token_type != TokenType::RPAREN
        {
            return toks;
        }
        // The opening paren must match the closing one, not an inner one.
        let mut depth = 0usize;
        for (j, t) in toks.iter().enumerate() {
            match t.token_type {
                TokenType::LPAREN => depth += 1,
                TokenType::RPAREN => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 && j != n - 1 {
                        return toks;
                    }
                }
                _ => {}
            }
        }
        toks = &toks[1..n - 1];
    }
}

/// Split a condition (outer parens peeled) into its top-level conjuncts,
/// each with its own outer parens peeled. `None` when a disjunction sits at
/// the top level: AND binds tighter than OR, so such a condition is not a
/// set of independent tests.
pub fn split_conjuncts(cond: &[Token]) -> Option<Vec<&[Token]>> {
    let cond = strip_outer_parens(cond);
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (j, t) in cond.iter().enumerate() {
        match t.token_type {
            TokenType::LPAREN => depth += 1,
            TokenType::RPAREN => depth = depth.saturating_sub(1),
            ty if depth == 0 && CONJUNCTION_OPS.contains(&ty) => {
                parts.push(strip_outer_parens(&cond[start..j]));
                start = j + 1;
            }
            ty if depth == 0 && DISJUNCTION_OPS.contains(&ty) => return None,
            _ => {}
        }
    }
    parts.push(strip_outer_parens(&cond[start..]));
    Some(parts)
}

/// The operands of a condition's `and` / `or` tree, each with its outer
/// parens peeled: `(a = 1 | b = 2) & c = 3` yields `a = 1`, `b = 2`,
/// `c = 3`. An operand that is anything other than an `and` / `or`
/// (a comparison, arithmetic, a `not …`) is a leaf and is not split.
pub fn logical_operands(cond: &[Token]) -> Vec<&[Token]> {
    let cond = strip_outer_parens(cond);
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (j, t) in cond.iter().enumerate() {
        match t.token_type {
            TokenType::LPAREN => depth += 1,
            TokenType::RPAREN => depth = depth.saturating_sub(1),
            ty if depth == 0
                && (CONJUNCTION_OPS.contains(&ty) || DISJUNCTION_OPS.contains(&ty)) =>
            {
                parts.push(&cond[start..j]);
                start = j + 1;
            }
            _ => {}
        }
    }
    if parts.is_empty() {
        return vec![cond];
    }
    parts.push(&cond[start..]);
    parts.into_iter().flat_map(logical_operands).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    /// `=` / `eq`, and `in (…)`: the variable equals one of the values.
    In,
    Lt,
    Le,
    Gt,
    Ge,
}

/// One literal operand of a comparison, with where it was written.
#[derive(Debug, Clone)]
pub struct Value<'a> {
    pub lit: Lit,
    /// The literal's first token (the `-` of a negative number).
    pub tok: &'a Token,
    /// Source text, e.g. `-1` or `'A'`.
    pub display: String,
}

/// `VAR op literal` or `VAR in (literal, …)`. A colon range in the list
/// is expanded to its members, each pointing at the range's first token.
#[derive(Debug, Clone)]
pub struct Comparison<'a> {
    pub var: &'a Token,
    pub op: CmpOp,
    pub values: Vec<Value<'a>>,
    /// All of the comparison's tokens.
    pub tokens: &'a [Token],
}

impl Comparison<'_> {
    /// The variable name, lowercased (SAS names are case-insensitive).
    pub fn var_key(&self) -> String {
        self.var.text.to_lowercase()
    }

    /// The comparison as written in `source` (`x = 4`, `e in (0,1,2)`).
    pub fn source_text(&self, source: &str) -> String {
        let start = self.var.start_byte as usize;
        let end = self.tokens.last().map_or(start, |t| t.end_byte as usize);
        source.get(start..end).unwrap_or_default().to_string()
    }
}

/// The most members a colon range may expand to. A wider range makes the
/// comparison unreadable rather than a set too large to hold.
const MAX_RANGE_MEMBERS: i64 = 1000;

/// Values grouped by the literal they were written as, in order: a colon
/// range's members share its first token. Each group is shown as the value
/// (`4`), a run of three or more (`5:10`) or a list (`2, 4`), with its size.
pub fn group_by_literal<'a>(values: &[&Value<'a>]) -> Vec<(&'a Token, String, usize)> {
    let mut groups: Vec<(&'a Token, Vec<&Value<'a>>)> = Vec::new();
    for v in values {
        match groups.last_mut() {
            Some((tok, members)) if tok.start_byte == v.tok.start_byte => members.push(v),
            _ => groups.push((v.tok, vec![v])),
        }
    }
    groups
        .into_iter()
        .map(|(tok, members)| {
            let ints: Vec<i64> = members
                .iter()
                .filter_map(|v| match v.lit {
                    Lit::Int(n) => Some(n),
                    _ => None,
                })
                .collect();
            let run = ints.len() == members.len()
                && ints.len() >= 3
                && ints.windows(2).all(|w| w[1] == w[0] + 1);
            let display = if run {
                format!("{}:{}", ints[0], ints[ints.len() - 1])
            } else {
                members
                    .iter()
                    .map(|v| v.display.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            (tok, display, members.len())
        })
        .collect()
}

/// Read `part` as a single comparison of a variable against literals. The
/// comparison must be all of `part`: `e in (9) + 1` or `e = 1 - x` is
/// arithmetic, not a set test. Prefix comparisons (`=:`, `in:`) and
/// negations are not equality tests and yield `None`.
pub fn comparison(part: &[Token]) -> Option<Comparison<'_>> {
    let var = part
        .first()
        .filter(|t| t.token_type == TokenType::Identifier)?;
    let op = part.get(1)?;
    let value = |k: usize| -> Option<(Value<'_>, usize)> {
        let (lit, used) = literal_at(part, k)?;
        let display = part[k..k + used].iter().map(|t| t.text.as_str()).collect();
        Some((
            Value {
                lit,
                tok: &part[k],
                display,
            },
            used,
        ))
    };
    let single = |op: CmpOp| -> Option<Comparison<'_>> {
        let (v, used) = value(2)?;
        (2 + used == part.len()).then(|| Comparison {
            var,
            op,
            values: vec![v],
            tokens: part,
        })
    };
    match op.token_type {
        TokenType::KwEQ | TokenType::ASSIGN => single(CmpOp::In),
        TokenType::KwLT | TokenType::LT => single(CmpOp::Lt),
        TokenType::KwLE | TokenType::LE => single(CmpOp::Le),
        TokenType::KwGT | TokenType::GT => single(CmpOp::Gt),
        TokenType::KwGE | TokenType::GE => single(CmpOp::Ge),
        TokenType::KwIN => {
            // `in (lit, lit, lo:hi, …)`, commas optional as SAS allows.
            if part.get(2)?.token_type != TokenType::LPAREN
                || part.last()?.token_type != TokenType::RPAREN
            {
                return None;
            }
            let close = part.len() - 1;
            let mut values = Vec::new();
            let mut k = 3;
            while k < close {
                if part[k].token_type == TokenType::COMMA {
                    k += 1;
                    continue;
                }
                let (v, used) = value(k)?;
                k += used;
                // `lo:hi` stands for the whole numbers from `lo` to `hi`.
                if part.get(k).map(|t| t.token_type) == Some(TokenType::COLON) {
                    let (hi, hi_used) = literal_at(part, k + 1)?;
                    let (Lit::Int(lo), Lit::Int(hi)) = (&v.lit, hi) else {
                        return None;
                    };
                    if hi < *lo || hi - lo >= MAX_RANGE_MEMBERS {
                        return None;
                    }
                    values.extend((*lo..=hi).map(|n| Value {
                        lit: Lit::Int(n),
                        tok: v.tok,
                        display: n.to_string(),
                    }));
                    k += 1 + hi_used;
                    continue;
                }
                values.push(v);
            }
            (!values.is_empty()).then_some(Comparison {
                var,
                op: CmpOp::In,
                values,
                tokens: part,
            })
        }
        _ => None,
    }
}
