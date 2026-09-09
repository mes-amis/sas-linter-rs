use std::collections::BTreeSet;

use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::Token;

use super::RuleMeta;

pub struct UnreachableElseIfBranch;

const ID: &str = "unreachable_else_if_branch";
const DESCRIPTION: &str = "An `else if` arm whose condition is already covered by an \
                            earlier arm of the same chain, so it can never fire.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        // Report-only by design: whether the shadowed arm is a leftover to
        // delete or a branch meant to test something else is the author's
        // call. The line number is the whole value.
        supports_autofix: false,
        default_factory: || Box::new(UnreachableElseIfBranch),
        config_factory: |_| Ok(Box::new(UnreachableElseIfBranch)),
    }
}

impl Rule for UnreachableElseIfBranch {
    fn id(&self) -> &'static str {
        ID
    }
    fn description(&self) -> &'static str {
        DESCRIPTION
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        let mut walker = Walker {
            tokens: &ctx.tokens.default,
            path: ctx.path,
            findings: Vec::new(),
        };
        walker.statements(0, false);
        walker.findings.sort_by_key(|f| (f.line, f.column));
        walker.findings
    }
}

// ── Statement walker ─────────────────────────────────────────────────────
//
// A small recursive-descent pass over the default token channel. It only
// needs to know where statements start and end, which `do` / `select`
// block an `end;` closes, and which `if` an `else` belongs to — enough to
// group the arms of one chain and to keep chains nested inside a
// `then do; … end;` arm separate from the chain around them.

struct Walker<'a> {
    tokens: &'a [Token],
    path: &'a str,
    findings: Vec<Finding>,
}

impl<'a> Walker<'a> {
    fn ty(&self, i: usize) -> Option<TokenType> {
        self.tokens.get(i).map(|t| t.token_type)
    }

    /// Parse statements from `i`. Stops at EOF, or — when `in_block` — at a
    /// block-closing `end;`, whose index is returned for the caller to eat.
    fn statements(&mut self, mut i: usize, in_block: bool) -> usize {
        while i < self.tokens.len() {
            if in_block && self.closes_block(i) {
                return i;
            }
            i = self.statement(i);
        }
        self.tokens.len()
    }

    /// Parse one statement starting at `i`; return the index just past it.
    fn statement(&mut self, i: usize) -> usize {
        let Some(tok) = self.tokens.get(i) else {
            return self.tokens.len();
        };
        match tok.token_type {
            TokenType::KwIf => self.chain(i),
            TokenType::KwDo | TokenType::KwSelect => self.block(i),
            // A stray `else` (one whose `if` we never saw, e.g. a fragment
            // that opens mid-chain) still owns the statement after it.
            TokenType::KwElse => self.statement(i + 1),
            // `when (…) stmt` inside a select block.
            TokenType::KwWhen if self.ty(i + 1) == Some(TokenType::LPAREN) => {
                let after = self.skip_parens(i + 1);
                self.statement(after)
            }
            TokenType::Identifier if tok.text.eq_ignore_ascii_case("otherwise") => {
                self.statement(i + 1)
            }
            _ => self.skip_statement(i),
        }
    }

    /// `do …;` / `select …;` header through its matching `end;`.
    fn block(&mut self, i: usize) -> usize {
        let body = self.skip_statement(i);
        let end = self.statements(body, true);
        // Past `end` and its `;` — or EOF if the block never closed.
        (end + 2).min(self.tokens.len())
    }

    /// A block-closing `end` is always the whole statement: `end;`. That
    /// keeps `set b end = eof;` and a variable named `end` from counting.
    fn closes_block(&self, i: usize) -> bool {
        self.ty(i) == Some(TokenType::KwEnd) && self.ty(i + 1) == Some(TokenType::SEMI)
    }

    /// Index just past the next `;` at or after `i` (or EOF).
    fn skip_statement(&self, i: usize) -> usize {
        self.tokens[i..]
            .iter()
            .position(|t| t.token_type == TokenType::SEMI)
            .map(|p| i + p + 1)
            .unwrap_or(self.tokens.len())
    }

    /// `i` is a `(`; return the index just past its matching `)`.
    fn skip_parens(&self, i: usize) -> usize {
        let mut depth = 0usize;
        for (j, t) in self.tokens.iter().enumerate().skip(i) {
            match t.token_type {
                TokenType::LPAREN => depth += 1,
                TokenType::RPAREN => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return j + 1;
                    }
                }
                TokenType::SEMI => return j,
                _ => {}
            }
        }
        self.tokens.len()
    }

    /// One `if` / `else if` / `else` chain starting at the `if` at `i`.
    /// Returns the index just past the whole chain.
    fn chain(&mut self, mut i: usize) -> usize {
        let mut arms: Vec<Arm> = Vec::new();
        loop {
            // tokens[i] is the `if` of the current arm.
            let Some(then_idx) = self.find_then(i + 1) else {
                // No `then` before the `;` — malformed_if_condition's
                // department. Skip the statement and abandon the chain.
                return self.skip_statement(i);
            };
            let if_tok = &self.tokens[i];
            if let Some(arm) = analyze_condition(&self.tokens[i + 1..then_idx]) {
                if let Some((shadow, identical)) = shadowing_arm(&arms, &arm) {
                    self.findings.push(Finding {
                        path: self.path.to_string(),
                        line: if_tok.start_line,
                        column: if_tok.start_column + 1,
                        rule: ID,
                        message: format!(
                            "condition is {} the branch at line {}, so this branch can never fire",
                            if identical {
                                "identical to"
                            } else {
                                "fully covered by"
                            },
                            shadow.line
                        ),
                        severity: Severity::Warning,
                    });
                }
                arms.push(Arm {
                    line: if_tok.start_line,
                    conjuncts: arm,
                });
            }

            i = self.statement(then_idx + 1);
            if self.ty(i) != Some(TokenType::KwElse) {
                return i;
            }
            if self.ty(i + 1) == Some(TokenType::KwIf) {
                i += 1;
                continue;
            }
            // Bare `else`: the chain's terminal arm. Never reported.
            return self.statement(i + 1);
        }
    }

    /// Index of the `then` closing the condition that starts at `i`, or
    /// `None` if a `;` (or EOF) arrives first.
    fn find_then(&self, i: usize) -> Option<usize> {
        for (j, t) in self.tokens.iter().enumerate().skip(i) {
            match t.token_type {
                TokenType::KwThen => return Some(j),
                TokenType::SEMI => return None,
                _ => {}
            }
        }
        None
    }
}

// ── Condition model ──────────────────────────────────────────────────────
//
// An arm's condition is split on top-level `and` into conjuncts. Each
// conjunct is either something the rule can reason about — a set test or a
// one-sided range on a single variable — or an opaque token sequence that
// only ever matches itself exactly.

struct Arm {
    line: u32,
    conjuncts: Vec<Conjunct>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Lit {
    Int(i64),
    /// Non-integral float, by bit pattern (so it can sit in a BTreeSet).
    Float(u64),
    /// String literal, quotes stripped. Case is significant.
    Str(String),
}

impl Lit {
    fn as_f64(&self) -> Option<f64> {
        match self {
            Lit::Int(n) => Some(*n as f64),
            Lit::Float(bits) => Some(f64::from_bits(*bits)),
            Lit::Str(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Bound {
    Unbounded,
    Inclusive(f64),
    Exclusive(f64),
}

#[derive(Debug, Clone)]
enum Conjunct {
    /// `VAR in (…)`, or `VAR = lit` / `VAR eq lit` as a one-element set.
    Set { var: String, values: BTreeSet<Lit> },
    /// `VAR lt/le/gt/ge N` — a half-line.
    Range { var: String, lo: Bound, hi: Bound },
    /// Anything else, as lowercased token text. Matches only itself.
    Opaque(Vec<String>),
}

/// Tokens that make the whole condition opaque to conjunct splitting:
/// `and` binds tighter than `or`, so a top-level disjunction can't be
/// treated as an unordered set of conjuncts.
const DISJUNCTION_OPS: &[TokenType] = &[
    TokenType::KwOR,
    TokenType::PIPE,
    TokenType::PIPE2,
    TokenType::EXCL,
    TokenType::EXCL2,
    TokenType::AMP,
];

/// Model an arm's condition. `None` means the arm is fully opaque — it is
/// neither reported nor used to shadow later arms. That is the case for an
/// empty condition and for any condition containing a function call:
/// `lag(x)` and `rand(…)` return different values on each call, so a
/// repeated condition is not a repeated test.
fn analyze_condition(cond: &[Token]) -> Option<Vec<Conjunct>> {
    let cond = strip_outer_parens(cond);
    if cond.is_empty() {
        return None;
    }
    let is_call = cond
        .windows(2)
        .any(|w| w[0].token_type == TokenType::Identifier && w[1].token_type == TokenType::LPAREN);
    if is_call {
        return None;
    }

    let mut conjuncts = Vec::new();
    for part in split_top_level_and(cond) {
        conjuncts.push(classify(strip_outer_parens(part)));
    }
    Some(conjuncts)
}

/// Split on `and` at paren depth 0. A top-level disjunction operator
/// disables splitting: the whole condition becomes one part.
fn split_top_level_and(cond: &[Token]) -> Vec<&[Token]> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (j, t) in cond.iter().enumerate() {
        match t.token_type {
            TokenType::LPAREN => depth += 1,
            TokenType::RPAREN => depth = depth.saturating_sub(1),
            TokenType::KwAND if depth == 0 => {
                parts.push(&cond[start..j]);
                start = j + 1;
            }
            ty if depth == 0 && DISJUNCTION_OPS.contains(&ty) => return vec![cond],
            _ => {}
        }
    }
    parts.push(&cond[start..]);
    parts
}

/// Peel `( … )` pairs that enclose the entire slice.
fn strip_outer_parens(mut toks: &[Token]) -> &[Token] {
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
                    depth -= 1;
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

fn classify(part: &[Token]) -> Conjunct {
    if let Some(c) = classify_set(part).or_else(|| classify_range(part)) {
        return c;
    }
    Conjunct::Opaque(
        part.iter()
            .map(|t| match t.token_type {
                // Case inside a string literal is significant.
                TokenType::StringLiteral => t.text.clone(),
                _ => t.text.to_lowercase(),
            })
            .collect(),
    )
}

/// `VAR in (lit, lit, …)` (commas optional, as SAS allows) or
/// `VAR = lit` / `VAR eq lit`.
fn classify_set(part: &[Token]) -> Option<Conjunct> {
    let var = part
        .first()
        .filter(|t| t.token_type == TokenType::Identifier)?;
    let op = part.get(1)?;
    let mut values = BTreeSet::new();
    match op.token_type {
        TokenType::KwEQ | TokenType::ASSIGN => {
            let (lit, used) = literal_at(part, 2)?;
            if 2 + used != part.len() {
                return None;
            }
            values.insert(lit);
        }
        TokenType::KwIN => {
            if part.get(2)?.token_type != TokenType::LPAREN
                || part.last()?.token_type != TokenType::RPAREN
            {
                return None;
            }
            let mut k = 3;
            let close = part.len() - 1;
            while k < close {
                if part[k].token_type == TokenType::COMMA {
                    k += 1;
                    continue;
                }
                let (lit, used) = literal_at(part, k)?;
                values.insert(lit);
                k += used;
            }
            if values.is_empty() {
                return None;
            }
        }
        _ => return None,
    }
    Some(Conjunct::Set {
        var: var.text.to_lowercase(),
        values,
    })
}

/// `VAR lt N` / `VAR le N` / `VAR gt N` / `VAR ge N` and the symbolic forms.
fn classify_range(part: &[Token]) -> Option<Conjunct> {
    let var = part
        .first()
        .filter(|t| t.token_type == TokenType::Identifier)?;
    let op = part.get(1)?;
    let (lit, used) = literal_at(part, 2)?;
    if 2 + used != part.len() {
        return None;
    }
    let n = lit.as_f64()?;
    let (lo, hi) = match op.token_type {
        TokenType::KwLT | TokenType::LT => (Bound::Unbounded, Bound::Exclusive(n)),
        TokenType::KwLE | TokenType::LE => (Bound::Unbounded, Bound::Inclusive(n)),
        TokenType::KwGT | TokenType::GT => (Bound::Exclusive(n), Bound::Unbounded),
        TokenType::KwGE | TokenType::GE => (Bound::Inclusive(n), Bound::Unbounded),
        _ => return None,
    };
    Some(Conjunct::Range {
        var: var.text.to_lowercase(),
        lo,
        hi,
    })
}

/// A literal at `k`, allowing a leading unary minus. Returns the value and
/// the number of tokens consumed.
fn literal_at(part: &[Token], k: usize) -> Option<(Lit, usize)> {
    let t = part.get(k)?;
    match t.token_type {
        TokenType::MINUS => {
            let (lit, used) = literal_at(part, k + 1)?;
            let neg = match lit {
                Lit::Int(n) => Lit::Int(-n),
                Lit::Float(bits) => num_lit(-f64::from_bits(bits)),
                Lit::Str(_) => return None,
            };
            Some((neg, used + 1))
        }
        TokenType::IntegerLiteral | TokenType::FloatLiteral | TokenType::FloatExponentLiteral => {
            let f: f64 = t.text.parse().ok()?;
            Some((num_lit(f), 1))
        }
        TokenType::StringLiteral => {
            let s = t.text.as_str();
            let inner = if s.len() >= 2
                && (s.starts_with('\'') && s.ends_with('\'')
                    || s.starts_with('"') && s.ends_with('"'))
            {
                &s[1..s.len() - 1]
            } else {
                s
            };
            Some((Lit::Str(inner.to_string()), 1))
        }
        _ => None,
    }
}

fn num_lit(f: f64) -> Lit {
    if f.is_finite() && f == f.trunc() && f.abs() < i64::MAX as f64 {
        Lit::Int(f as i64)
    } else {
        Lit::Float(f.to_bits())
    }
}

// ── Reasoning ────────────────────────────────────────────────────────────

/// The earliest arm that makes `later` unreachable, and whether the two
/// conditions are equivalent (as opposed to `later` being strictly
/// narrower). Prefers an identical arm when there is one, since "identical
/// to line N" is the more useful message.
fn shadowing_arm<'a>(arms: &'a [Arm], later: &[Conjunct]) -> Option<(&'a Arm, bool)> {
    let mut covering = None;
    for arm in arms {
        if covers(&arm.conjuncts, later) {
            if covers(later, &arm.conjuncts) {
                return Some((arm, true));
            }
            covering.get_or_insert((arm, false));
        }
    }
    covering
}

/// `later ⇒ earlier`: every conjunct of the earlier arm is implied by some
/// conjunct of the later one, so whenever the later arm would be true the
/// earlier arm already fired.
fn covers(earlier: &[Conjunct], later: &[Conjunct]) -> bool {
    earlier.iter().all(|e| later.iter().any(|l| implies(l, e)))
}

/// Does conjunct `l` being true guarantee conjunct `e` is true?
fn implies(l: &Conjunct, e: &Conjunct) -> bool {
    match (l, e) {
        (Conjunct::Opaque(a), Conjunct::Opaque(b)) => a == b,
        (
            Conjunct::Set {
                var: lv,
                values: lvals,
            },
            Conjunct::Set {
                var: ev,
                values: evals,
            },
        ) => lv == ev && lvals.is_subset(evals),
        (
            Conjunct::Set {
                var: lv,
                values: lvals,
            },
            Conjunct::Range { var: ev, lo, hi },
        ) => {
            lv == ev
                && lvals
                    .iter()
                    .all(|v| v.as_f64().is_some_and(|n| in_range(n, *lo, *hi)))
        }
        (
            Conjunct::Range {
                var: lv,
                lo: llo,
                hi: lhi,
            },
            Conjunct::Range {
                var: ev,
                lo: elo,
                hi: ehi,
            },
        ) => lv == ev && lo_covers(*elo, *llo) && hi_covers(*ehi, *lhi),
        _ => false,
    }
}

fn in_range(n: f64, lo: Bound, hi: Bound) -> bool {
    let above = match lo {
        Bound::Unbounded => true,
        Bound::Inclusive(a) => n >= a,
        Bound::Exclusive(a) => n > a,
    };
    let below = match hi {
        Bound::Unbounded => true,
        Bound::Inclusive(b) => n <= b,
        Bound::Exclusive(b) => n < b,
    };
    above && below
}

/// Is every value admitted by lower bound `inner` also admitted by `outer`?
fn lo_covers(outer: Bound, inner: Bound) -> bool {
    match (outer, inner) {
        (Bound::Unbounded, _) => true,
        (_, Bound::Unbounded) => false,
        (Bound::Inclusive(a), Bound::Inclusive(b)) => a <= b,
        (Bound::Inclusive(a), Bound::Exclusive(b)) => a <= b,
        (Bound::Exclusive(a), Bound::Inclusive(b)) => a < b,
        (Bound::Exclusive(a), Bound::Exclusive(b)) => a <= b,
    }
}

/// Is every value admitted by upper bound `inner` also admitted by `outer`?
fn hi_covers(outer: Bound, inner: Bound) -> bool {
    match (outer, inner) {
        (Bound::Unbounded, _) => true,
        (_, Bound::Unbounded) => false,
        (Bound::Inclusive(a), Bound::Inclusive(b)) => a >= b,
        (Bound::Inclusive(a), Bound::Exclusive(b)) => a >= b,
        (Bound::Exclusive(a), Bound::Inclusive(b)) => a > b,
        (Bound::Exclusive(a), Bound::Exclusive(b)) => a >= b,
    }
}
