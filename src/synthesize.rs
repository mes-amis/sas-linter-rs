//! Synthesise SAS-input testdata by walking the source's logical graph
//! with the SAS lexer and sampling from the distribution it induces.
//! Backs `sas-lint synthesize`.
//!
//! The graph is built in three layers:
//!
//!   1. **Inputs** — identifiers the algorithm reads. Discovered by
//!      unioning (a) `VAR = VAR;` self-declarations, (b) identifiers used
//!      in any `if`-head atom but never assigned, and (c) the member
//!      columns of every `array …;` declaration. Names of arrays
//!      themselves are removed from the input set.
//!
//!   2. **Predicates** — every `if … then` head, parsed into a [`Pred`]
//!      tree. Compound predicates retain their boolean structure so the
//!      sampler can satisfy them coherently (an OR picks one disjunct, an
//!      AND satisfies all conjuncts).
//!
//!   3. **Value sets** — for each input, the alphabet the sampler draws
//!      from: literal values from atoms, acceptable-values metadata, plus
//!      a 0-or-empty default. Sum contributors additionally seed
//!      `0..=threshold`.
//!
//! Each row is drawn from one of five strategies (see [`Strategy`]), mixed
//! by [`Mix`]. Sampling is deterministic for a given seed.
//!
//! [`minimal`] is the alternative to weighted sampling: it builds the
//! smallest row set it can find that drives every `if` both ways (100%
//! branch coverage of the modelled graph), ordered by the first branch
//! each row exercises in source order.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::Write;
use std::ops::RangeInclusive;
use std::path::Path;
use std::str::FromStr;

use sas_lexer::TokenType;

use crate::token::{Token, TokenStream};
use crate::value_specs::{SpecKind, ValueSpecs};

pub const DEFAULT_SIZE: usize = 500;
pub const DEFAULT_SEED: u64 = 42;

/// Default per-row strategy mix. Tuned to get balanced coverage of "no
/// trigger" vs "single trigger" vs "compound triggers" — uniform random
/// rows tend to trip many criteria at once and wash out the rarer outcome
/// classes.
const STRATEGY_WEIGHTS: [(Strategy, f64); 5] = [
    (Strategy::Baseline, 0.15),
    (Strategy::Single, 0.45),
    (Strategy::Combo, 0.25),
    (Strategy::Perturbation, 0.10),
    (Strategy::Chaos, 0.05),
];
/// Default per-cell deviate probability inside `Perturbation` rows.
pub const DEFAULT_PERTURBATION_PROB: f64 = 0.05;
/// Default number of distinct predicates fired per `Combo` row. K∈{2..6}
/// roughly doubles cumulative-sum coverage versus K≤4 while preserving
/// coverage of rare output classes; higher K starts collapsing them
/// because mutually exclusive guards conflict and first-writer-wins
/// biases rows into the dominant branch.
const COMBO_SIZES: RangeInclusive<usize> = 2..=6;
/// Random re-tries per placement when [`minimal`] packs a branch outcome
/// into a row (OR disjuncts, array members, and value picks are random).
const MINIMAL_ATTEMPTS: usize = 16;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Strategy {
    /// Everything at default — exercise the "nothing fired" branch.
    Baseline,
    /// Satisfy one predicate, leave the rest at default.
    Single,
    /// Satisfy K predicates at once — cumulative-sum / multi-criterion paths.
    Combo,
    /// Baseline + flip a small handful of cells to random.
    Perturbation,
    /// Pure uniform sample over each input's value set.
    Chaos,
}

impl Strategy {
    pub const ALL: [Strategy; 5] = [
        Strategy::Baseline,
        Strategy::Single,
        Strategy::Combo,
        Strategy::Perturbation,
        Strategy::Chaos,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Strategy::Baseline => "baseline",
            Strategy::Single => "single",
            Strategy::Combo => "combo",
            Strategy::Perturbation => "perturbation",
            Strategy::Chaos => "chaos",
        }
    }
}

/// Relative strategy weights. Parsed from `name=weight,…`; strategies not
/// named get weight 0, and weights need not sum to 1.
#[derive(Clone, Debug, PartialEq)]
pub struct Weights([(Strategy, f64); 5]);

impl Default for Weights {
    fn default() -> Self {
        Weights(STRATEGY_WEIGHTS)
    }
}

impl fmt::Display for Weights {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, (s, w)) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(",")?;
            }
            write!(f, "{}={w}", s.name())?;
        }
        Ok(())
    }
}

impl FromStr for Weights {
    type Err = String;

    fn from_str(spec: &str) -> Result<Self, String> {
        let mut weights = Strategy::ALL.map(|s| (s, 0.0));
        let mut seen = HashSet::new();
        for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (name, w) = part
                .split_once('=')
                .ok_or_else(|| format!("expected `strategy=weight`, got `{part}`"))?;
            let name = name.trim();
            let slot = weights
                .iter_mut()
                .find(|(s, _)| s.name().eq_ignore_ascii_case(name))
                .ok_or_else(|| {
                    let names: Vec<&str> = Strategy::ALL.iter().map(|s| s.name()).collect();
                    format!(
                        "unknown strategy `{name}` (expected one of {})",
                        names.join(", ")
                    )
                })?;
            if !seen.insert(slot.0) {
                return Err(format!("strategy `{name}` given more than once"));
            }
            slot.1 = match w.trim().parse::<f64>() {
                Ok(x) if x.is_finite() && x >= 0.0 => x,
                _ => return Err(format!("weight for `{name}` must be a non-negative number")),
            };
        }
        if weights.iter().map(|(_, w)| w).sum::<f64>() <= 0.0 {
            return Err("at least one strategy needs a positive weight".into());
        }
        Ok(Weights(weights))
    }
}

/// Inclusive `MIN-MAX` (or a bare `N`) count of predicates per combo row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComboSizes(pub RangeInclusive<usize>);

impl Default for ComboSizes {
    fn default() -> Self {
        ComboSizes(COMBO_SIZES)
    }
}

impl fmt::Display for ComboSizes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.0.start(), self.0.end())
    }
}

impl FromStr for ComboSizes {
    type Err = String;

    fn from_str(spec: &str) -> Result<Self, String> {
        let num = |s: &str| {
            s.trim()
                .parse::<usize>()
                .ok()
                .filter(|n| *n >= 1)
                .ok_or_else(|| format!("`{}` is not a positive integer", s.trim()))
        };
        let (lo, hi) = match spec.split_once('-') {
            Some((lo, hi)) => (num(lo)?, num(hi)?),
            None => {
                let n = num(spec)?;
                (n, n)
            }
        };
        if lo > hi {
            return Err(format!("combo size range {lo}-{hi} is empty"));
        }
        Ok(ComboSizes(lo..=hi))
    }
}

/// Knobs for [`sample_with`]: the strategy mix plus the per-strategy
/// parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct Mix {
    pub weights: Weights,
    /// Per-cell deviate probability inside `Perturbation` rows, in `[0, 1]`.
    pub perturbation_prob: f64,
    pub combo_sizes: ComboSizes,
}

impl Default for Mix {
    fn default() -> Self {
        Mix {
            weights: Weights::default(),
            perturbation_prob: DEFAULT_PERTURBATION_PROB,
            combo_sizes: ComboSizes::default(),
        }
    }
}

// ── Values ──────────────────────────────────────────────────────────────

/// A cell value. Equality is structural, so `Int(1) != Float(1.0)` when
/// de-duplicating value sets.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Str(String),
    Missing,
}

impl Value {
    fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(n) => Some(*n as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    fn is_numeric(&self) -> bool {
        self.as_f64().is_some()
    }

    fn to_i64(&self) -> i64 {
        match self {
            Value::Int(n) => *n,
            Value::Float(f) => f.trunc() as i64,
            _ => 0,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(n) => write!(f, "{n}"),
            // Keep a trailing `.0` on whole floats so they read as floats.
            Value::Float(x) if x.is_finite() && x.fract() == 0.0 && x.abs() < 1e16 => {
                write!(f, "{x:.1}")
            }
            Value::Float(x) => write!(f, "{x}"),
            Value::Str(s) => f.write_str(s),
            Value::Missing => Ok(()),
        }
    }
}

// ── Graph ───────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Op {
    In,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Debug)]
struct Atom {
    var: String,
    op: Op,
    values: Vec<Value>,
    is_array: bool,
}

#[derive(Clone, Debug)]
struct SumAtom {
    vars: Vec<String>,
    op: Op,
    value: Value,
}

#[derive(Clone, Debug)]
enum Pred {
    Atom(Atom),
    Sum(SumAtom),
    And(Vec<Pred>),
    Or(Vec<Pred>),
    Not(Box<Pred>),
    Tautology,
}

#[derive(Clone, Debug)]
struct ArrayDecl {
    members: Vec<String>,
    char: bool,
}

/// One `if` decision point, in source (execution) order. `reach` is every
/// condition that must hold for control to arrive at the `if`: enclosing
/// `then do` guards, the negated earlier arms of an `else if` chain, and
/// any subsetting `if X;` earlier in the same block.
#[derive(Clone, Debug)]
struct Branch {
    line: u32,
    reach: Vec<Pred>,
    leaf: Pred,
}

/// The source's logical graph — inputs, predicates, and per-input value sets.
#[derive(Debug)]
pub struct Graph {
    pub inputs: Vec<String>,
    predicates: Vec<Pred>,
    /// Decision points whose own condition reads an input; the targets
    /// [`minimal`] covers.
    branches: Vec<Branch>,
    arrays: HashMap<String, ArrayDecl>,
    value_sets: HashMap<String, Vec<Value>>,
    default_row: HashMap<String, Value>,
}

/// Sampled rows, one cell per header.
#[derive(Debug)]
pub struct Synthesized {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

impl Synthesized {
    pub fn write_csv<W: Write>(&self, mut out: W) -> csv::Result<()> {
        // The csv crate renders a zero-field record as `""`; emit bare
        // newlines instead.
        if self.headers.is_empty() {
            for _ in 0..=self.rows.len() {
                out.write_all(b"\n")?;
            }
            return Ok(out.flush()?);
        }
        let mut w = csv::Writer::from_writer(out);
        w.write_record(&self.headers)?;
        for row in &self.rows {
            w.write_record(row.iter().map(|v| v.to_string()))?;
        }
        w.flush()?;
        Ok(())
    }
}

/// Sample `size` rows from `source`'s logical graph with the default
/// [`Mix`]. Deterministic for a given seed.
pub fn sample(source: &str, specs: &ValueSpecs, size: usize, seed: u64) -> Synthesized {
    sample_with(source, specs, size, seed, &Mix::default())
}

/// Sample `size` rows from `source`'s logical graph, drawing each row's
/// strategy from `mix`. Deterministic for a given seed.
pub fn sample_with(
    source: &str,
    specs: &ValueSpecs,
    size: usize,
    seed: u64,
    mix: &Mix,
) -> Synthesized {
    let g = build_graph(source, specs);
    let mut rng = Rng::new(seed);
    let rows = (0..size)
        .map(|_| cells(&g, &sample_row(&g, mix, &mut rng)))
        .collect();
    Synthesized {
        headers: g.inputs.clone(),
        rows,
    }
}

/// Branch outcomes [`minimal`] reached, out of every `if` true/false
/// outcome in the modelled graph.
#[derive(Debug, Default)]
pub struct Coverage {
    pub total: usize,
    /// `(source line, outcome)` of each outcome no row reaches — the
    /// branch is infeasible, or its condition is outside what the sampler
    /// can pin.
    pub uncovered: Vec<(u32, bool)>,
}

impl Coverage {
    pub fn covered(&self) -> usize {
        self.total - self.uncovered.len()
    }
}

/// The smallest row set found that drives every modelled `if` both ways,
/// ordered by the first branch outcome each row reaches in source order.
///
/// Greedy set cover: outcomes are visited in source order, and each one
/// not already reached is packed into the earliest row whose pins it
/// doesn't disturb, else into a fresh row; rows left redundant at the end
/// are dropped. Unparsed conditions are assumed to hold. Deterministic
/// for a given seed.
pub fn minimal(source: &str, specs: &ValueSpecs, seed: u64) -> (Synthesized, Coverage) {
    struct Candidate {
        pins: Row,
        row: Row,
        /// Outcomes this row is responsible for; later merges must keep them.
        keep: Vec<usize>,
    }

    let g = build_graph(source, specs);
    let mut rng = Rng::new(seed);
    let targets: Vec<(usize, bool)> = (0..g.branches.len())
        .flat_map(|b| [(b, true), (b, false)])
        .collect();
    let hit = |row: &Row, t: usize| {
        let (b, outcome) = targets[t];
        reaches(&g.branches[b], outcome, row, &g)
    };

    let mut cands: Vec<Candidate> = Vec::new();
    for (t, &(b, outcome)) in targets.iter().enumerate() {
        if let Some(c) = cands.iter_mut().find(|c| hit(&c.row, t)) {
            c.keep.push(t);
            continue;
        }
        let need = requirement(&g.branches[b], outcome);
        // Odd attempts pin outer conditions first, for when they're the
        // tighter bound.
        let mut reversed = need.clone();
        if let Pred::And(c) = &mut reversed {
            c.reverse();
        }
        'place: for ci in 0..=cands.len() {
            for attempt in 0..MINIMAL_ATTEMPTS {
                let base = cands.get(ci).map(|c| c.pins.clone()).unwrap_or_default();
                let mut pins = base.clone();
                let need =
                    pick_disjuncts(if attempt % 2 == 0 { &need } else { &reversed }, &mut rng);
                satisfy(&need, &mut pins, &g, &mut rng);
                repair(&need, &mut pins, &base, &g);
                let row = materialize(&g, &pins);
                let keep = cands.get(ci).map_or(&[][..], |c| &c.keep[..]);
                if !hit(&row, t) || !keep.iter().all(|&k| hit(&row, k)) {
                    continue;
                }
                match cands.get_mut(ci) {
                    Some(c) => {
                        c.pins = pins;
                        c.row = row;
                        c.keep.push(t);
                    }
                    None => cands.push(Candidate {
                        pins,
                        row,
                        keep: vec![t],
                    }),
                }
                break 'place;
            }
        }
    }

    // Drop rows whose every outcome another surviving row also reaches.
    let reached: Vec<Vec<usize>> = cands
        .iter()
        .map(|c| (0..targets.len()).filter(|&t| hit(&c.row, t)).collect())
        .collect();
    let mut alive = vec![true; cands.len()];
    for i in (0..cands.len()).rev() {
        let redundant = reached[i]
            .iter()
            .all(|t| (0..cands.len()).any(|j| j != i && alive[j] && reached[j].contains(t)));
        if redundant {
            alive[i] = false;
        }
    }
    let mut order: Vec<usize> = (0..cands.len()).filter(|&i| alive[i]).collect();
    // Source order of the outcomes each row reaches, earliest first.
    order.sort_by(|&a, &b| reached[a].cmp(&reached[b]));

    let uncovered = (0..targets.len())
        .filter(|t| !order.iter().any(|&i| reached[i].contains(t)))
        .map(|t| (g.branches[targets[t].0].line, targets[t].1))
        .collect();
    let mut rows: Vec<Vec<Value>> = order.iter().map(|&i| cells(&g, &cands[i].row)).collect();
    if rows.is_empty() {
        rows.push(cells(&g, &g.default_row));
    }
    (
        Synthesized {
            headers: g.inputs.clone(),
            rows,
        },
        Coverage {
            total: targets.len(),
            uncovered,
        },
    )
}

/// `node` with each OR collapsed to one randomly chosen disjunct, so the
/// result is a plain conjunction [`repair`] sees whole.
fn pick_disjuncts(node: &Pred, rng: &mut Rng) -> Pred {
    match node {
        Pred::Or(c) => rng
            .choose(c)
            .map_or(Pred::Tautology, |p| pick_disjuncts(p, rng)),
        Pred::And(c) => Pred::And(c.iter().map(|p| pick_disjuncts(p, rng)).collect()),
        other => other.clone(),
    }
}

/// Re-pick cells that [`satisfy`]'s first-writer-wins left violating a
/// conjunct of `need` — e.g. `a ne 0` (an earlier `else` arm) alongside
/// `a ne 1` needs an `a` outside both lists. Cells already in `base` (a
/// row's earlier commitments) stay put.
fn repair(need: &Pred, pins: &mut Row, base: &Row, g: &Graph) {
    fn conjuncts<'p>(node: &'p Pred, out: &mut Vec<&'p Atom>) {
        match node {
            Pred::Atom(a) if !a.is_array => out.push(a),
            Pred::And(c) => c.iter().for_each(|p| conjuncts(p, out)),
            _ => {}
        }
    }
    let mut atoms = Vec::new();
    conjuncts(need, &mut atoms);
    let mut by_var: HashMap<&str, Vec<&Atom>> = HashMap::new();
    for a in atoms {
        by_var.entry(a.var.as_str()).or_default().push(a);
    }
    let mut vars: Vec<&str> = by_var.keys().copied().collect();
    vars.sort_unstable();
    for var in vars {
        let Some(current) = g.default_row.get(var) else {
            continue;
        };
        let atoms = &by_var[var];
        let ok = |v: &Value| {
            atoms
                .iter()
                .all(|a| holds(a.op, v, &a.values) != Some(false))
        };
        if base.contains_key(var) || ok(pins.get(var).unwrap_or(current)) {
            continue;
        }
        let empty = Vec::new();
        let known = g.value_sets.get(var).unwrap_or(&empty);
        let literals: Vec<Value> = atoms
            .iter()
            .flat_map(|a| a.values.iter().cloned())
            .collect();
        let mut pool: Vec<Value> = known.iter().chain(&literals).cloned().collect();
        for v in &literals {
            pool.extend([bump(v.clone(), -1), bump(v.clone(), 1)]);
        }
        pool.push(outside(&literals, known));
        if let Some(v) = pool.into_iter().find(|v| ok(v)) {
            pins.insert(var.to_string(), v);
        }
    }
}

fn cells(g: &Graph, row: &Row) -> Vec<Value> {
    g.inputs
        .iter()
        .map(|h| row.get(h).cloned().unwrap_or(Value::Missing))
        .collect()
}

/// Output stem for a source path: the file name minus a `.sas` / `.txt`
/// extension (case-insensitive).
pub fn output_stem(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".sas") || lower.ends_with(".txt") {
        name[..name.len() - 4].to_string()
    } else {
        name
    }
}

pub fn build_graph(source: &str, specs: &ValueSpecs) -> Graph {
    let tokens = TokenStream::tokenize(source).default;
    let arrays = collect_array_decls(&tokens);
    let self_declared = collect_self_declared(&tokens);
    let assigned = collect_assigned(&tokens);
    let mut walker = Walker {
        tokens: &tokens,
        arrays: &arrays,
        used: HashSet::new(),
        preds: Vec::new(),
        branches: Vec::new(),
    };
    walker.walk(0, &[], &[]);
    let Walker {
        used,
        preds: mut predicates,
        mut branches,
        ..
    } = walker;

    let mut base: HashSet<String> = self_declared;
    base.extend(used.difference(&assigned).cloned());
    for name in arrays.keys() {
        base.remove(name);
    }
    for decl in arrays.values() {
        base.extend(decl.members.iter().cloned());
    }
    let mut inputs: Vec<String> = base.into_iter().collect();
    inputs.sort();

    // Drop predicates that reference no actual inputs — typically final
    // classification cascades over derived intermediates we can't pin from
    // the input row. Picking them in single-branch mode produces a
    // baseline-equivalent row, which biases toward "nothing fired".
    let input_set: HashSet<&str> = inputs.iter().map(String::as_str).collect();
    predicates.retain(|p| references_input(p, &input_set, &arrays));
    branches.retain(|b| references_input(&b.leaf, &input_set, &arrays));

    let mut char_inputs: HashSet<String> = HashSet::new();
    for decl in arrays.values().filter(|d| d.char) {
        char_inputs.extend(decl.members.iter().cloned());
    }
    char_inputs.extend(infer_char_inputs(&predicates));

    let value_sets = build_value_sets(&inputs, &predicates, &arrays, specs, &char_inputs);
    let default_row = build_default_row(&inputs, &value_sets, &char_inputs);

    Graph {
        inputs,
        predicates,
        branches,
        arrays,
        value_sets,
        default_row,
    }
}

fn references_input(
    node: &Pred,
    inputs: &HashSet<&str>,
    arrays: &HashMap<String, ArrayDecl>,
) -> bool {
    match node {
        Pred::Atom(a) => match arrays.get(&a.var).filter(|_| a.is_array) {
            Some(decl) => decl.members.iter().any(|m| inputs.contains(m.as_str())),
            None => inputs.contains(a.var.as_str()),
        },
        Pred::Sum(s) => s.vars.iter().any(|v| inputs.contains(v.as_str())),
        Pred::And(c) | Pred::Or(c) => c.iter().any(|p| references_input(p, inputs, arrays)),
        Pred::Not(c) => references_input(c, inputs, arrays),
        Pred::Tautology => false,
    }
}

fn ty(tokens: &[Token], i: usize) -> Option<TokenType> {
    tokens.get(i).map(|t| t.token_type)
}

fn is(tokens: &[Token], i: usize, want: TokenType) -> bool {
    ty(tokens, i) == Some(want)
}

// ── Array declarations ──────────────────────────────────────────────────
// `array A[12] $col1-col12;`   range form (most common)
// `array A{12} $col1-col12;`   brace alternate
// `array A(12) col1-col12;`    paren alternate
// `array A[*] col1-col5;`      star length
// `array A[3] col1 col2 col3;` explicit list

fn collect_array_decls(tokens: &[Token]) -> HashMap<String, ArrayDecl> {
    let mut out = HashMap::new();
    let mut i = 0;
    while i < tokens.len() {
        if tokens[i].token_type == TokenType::KwArray {
            if let Some((name, decl, j)) = parse_array_decl(tokens, i + 1) {
                out.insert(name, decl);
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn parse_array_decl(tokens: &[Token], mut i: usize) -> Option<(String, ArrayDecl, usize)> {
    if !is(tokens, i, TokenType::Identifier) {
        return None;
    }
    let name = tokens[i].text.clone();
    i += 1;

    let close = match ty(tokens, i)? {
        TokenType::LBRACK => TokenType::RBRACK,
        TokenType::LPAREN => TokenType::RPAREN,
        TokenType::LCURLY => TokenType::RCURLY,
        _ => return None,
    };
    i += 1;
    while i < tokens.len() && tokens[i].token_type != close {
        i += 1;
    }
    if !is(tokens, i, close) {
        return None;
    }
    i += 1;

    let mut char = false;
    if is(tokens, i, TokenType::DOLLAR) {
        char = true;
        i += 1;
    }

    if !is(tokens, i, TokenType::Identifier) {
        return None;
    }
    let first = tokens[i].text.clone();
    i += 1;

    let members = if is(tokens, i, TokenType::MINUS) && is(tokens, i + 1, TokenType::Identifier) {
        let last = &tokens[i + 1].text;
        i += 2;
        expand_range_members(&first, last).unwrap_or_else(|| vec![first])
    } else {
        let mut list = vec![first];
        while is(tokens, i, TokenType::Identifier)
            && ty(tokens, i + 1).is_some_and(|t| t != TokenType::ASSIGN)
        {
            list.push(tokens[i].text.clone());
            i += 1;
        }
        list
    };

    while i < tokens.len() && tokens[i].token_type != TokenType::SEMI {
        i += 1;
    }
    if i < tokens.len() {
        i += 1;
    }
    Some((name, ArrayDecl { members, char }, i))
}

/// `col1`, `col12` → `col1` … `col12`. `None` unless both ends share a
/// non-empty prefix and carry a numeric suffix.
fn expand_range_members(first: &str, last: &str) -> Option<Vec<String>> {
    let split = |s: &str| -> Option<(String, u64)> {
        let digits = s.bytes().rev().take_while(u8::is_ascii_digit).count();
        if digits == 0 || digits == s.len() {
            return None;
        }
        let (base, num) = s.split_at(s.len() - digits);
        Some((base.to_string(), num.parse().ok()?))
    };
    let (b1, lo) = split(first)?;
    let (b2, hi) = split(last)?;
    if b1 != b2 || hi < lo {
        return None;
    }
    Some((lo..=hi).map(|n| format!("{b1}{n}")).collect())
}

// ── Identifier classification ───────────────────────────────────────────

/// Inputs declared via the `VAR = VAR;` self-assignment idiom.
fn collect_self_declared(tokens: &[Token]) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut i = 0;
    while i + 3 < tokens.len() {
        let (a, b, c, d) = (&tokens[i], &tokens[i + 1], &tokens[i + 2], &tokens[i + 3]);
        if a.token_type == TokenType::Identifier
            && b.token_type == TokenType::ASSIGN
            && c.token_type == TokenType::Identifier
            && d.token_type == TokenType::SEMI
            && a.text == c.text
        {
            out.insert(a.text.clone());
            i += 4;
        } else {
            i += 1;
        }
    }
    out
}

/// Identifiers ever written to in a statement context (LHS of `=` after a
/// statement boundary). Used to *exclude* derived intermediates from the
/// input set.
fn collect_assigned(tokens: &[Token]) -> HashSet<String> {
    let mut out = HashSet::new();
    for i in 1..tokens.len() {
        if tokens[i].token_type != TokenType::ASSIGN {
            continue;
        }
        let lhs = &tokens[i - 1];
        if lhs.token_type == TokenType::Identifier && statement_context(tokens, i - 1) {
            out.insert(lhs.text.clone());
        }
    }
    out
}

/// True iff walking back from `ident_idx` hits a statement-boundary token
/// before any predicate-opener.
fn statement_context(tokens: &[Token], ident_idx: usize) -> bool {
    for t in tokens[..ident_idx].iter().rev() {
        match t.token_type {
            TokenType::SEMI
            | TokenType::KwThen
            | TokenType::KwElse
            | TokenType::KwDo
            | TokenType::KwEnd => return true,
            TokenType::KwIf | TokenType::KwAND | TokenType::KwOR | TokenType::LPAREN => {
                return false
            }
            _ => {}
        }
    }
    true
}

// ── `if`-head predicates ────────────────────────────────────────────────

/// Token-stream walker collecting the sampler's predicates and
/// [`minimal`]'s branches in one pass.
struct Walker<'a> {
    tokens: &'a [Token],
    arrays: &'a HashMap<String, ArrayDecl>,
    used: HashSet<String>,
    preds: Vec<Pred>,
    branches: Vec<Branch>,
}

impl Walker<'_> {
    /// Walk the token stream tracking the chain of enclosing
    /// `if … then do; … end;` guards. Each captured predicate is the AND
    /// of every enclosing if-pred plus the leaf if-pred — so when the
    /// sampler pins a leaf, it also pins the outer guards required to
    /// reach it.
    ///
    /// `reach` additionally tracks `else` arms and subsetting `if`s for
    /// the branch list; it never feeds the sampler's predicates.
    ///
    /// Bare `do … end` blocks (loops, compound statements without an `if`
    /// head) descend without pushing onto the context. Returns the index
    /// of the closing `end` (or the stream length).
    fn walk(&mut self, mut i: usize, context: &[Pred], reach: &[Pred]) -> usize {
        let tokens = self.tokens;
        let mut reach = reach.to_vec();
        // Index of the `else` that would pair with the last `if` walked,
        // and the reach of that else arm.
        let mut else_at: Option<(usize, Vec<Pred>)> = None;
        // Reach for the statement after a pairing `else`.
        let mut arm: Option<Vec<Pred>> = None;
        while i < tokens.len() {
            match tokens[i].token_type {
                TokenType::KwIf => {
                    let in_else = arm.is_some();
                    let here = arm.take().unwrap_or_else(|| reach.clone());
                    let j = if_head_end(tokens, i + 1);
                    let pred = PredicateParser::new(&tokens[i + 1..j], self.arrays, &mut self.used)
                        .parse();
                    capture_predicate(&mut self.preds, context, &pred);
                    self.branches.push(Branch {
                        line: tokens[i].start_line,
                        reach: here.clone(),
                        leaf: pred.clone(),
                    });
                    let mut not_taken = here.clone();
                    not_taken.push(Pred::Not(Box::new(pred.clone())));
                    if is(tokens, j, TokenType::SEMI) {
                        // Subsetting `if X;` — rows failing X stop here.
                        if !in_else {
                            reach.push(pred);
                        }
                        i = j + 1;
                    } else if is(tokens, j, TokenType::KwThen) && is(tokens, j + 1, TokenType::KwDo)
                    {
                        let mut body = j + 2;
                        if is(tokens, body, TokenType::SEMI) {
                            body += 1;
                        }
                        let mut inner = context.to_vec();
                        inner.push(pred.clone());
                        let mut inner_reach = here;
                        inner_reach.push(pred);
                        let end = self.walk(body, &inner, &inner_reach);
                        let after = end + 1 + usize::from(is(tokens, end + 1, TokenType::SEMI));
                        else_at = Some((after, not_taken));
                        i = end + 1;
                    } else {
                        else_at = Some((statement_end(tokens, j + 1) + 1, not_taken));
                        i = j + 1;
                    }
                }
                TokenType::KwElse => {
                    arm = else_at.take().filter(|(at, _)| *at == i).map(|(_, r)| r);
                    i += 1;
                }
                TokenType::KwDo => {
                    let body_reach = arm.take().unwrap_or_else(|| reach.clone());
                    let mut body = i + 1;
                    if is(tokens, body, TokenType::SEMI) {
                        body += 1;
                    }
                    i = self.walk(body, context, &body_reach) + 1;
                }
                TokenType::SEMI => {
                    arm = None;
                    i += 1;
                }
                TokenType::KwEnd => return i,
                _ => i += 1,
            }
        }
        i
    }
}

/// Index of the depth-0 `;` ending the statement that starts at `start`.
fn statement_end(tokens: &[Token], start: usize) -> usize {
    let mut depth = 0i32;
    let mut j = start;
    while j < tokens.len() {
        match tokens[j].token_type {
            TokenType::LPAREN => depth += 1,
            TokenType::RPAREN => depth -= 1,
            TokenType::SEMI if depth == 0 => return j,
            _ => {}
        }
        j += 1;
    }
    j
}

fn capture_predicate(preds: &mut Vec<Pred>, context: &[Pred], leaf: &Pred) {
    let mut chain: Vec<Pred> = context
        .iter()
        .chain(std::iter::once(leaf))
        .filter(|p| !matches!(p, Pred::Tautology))
        .cloned()
        .collect();
    match chain.len() {
        0 => {}
        1 => preds.push(chain.pop().unwrap()),
        _ => preds.push(Pred::And(chain)),
    }
}

/// Index of the `then` closing the if-head starting at `start`, ignoring
/// `then`s nested inside parens. A depth-0 `;` also ends the head, so a
/// subsetting `if X;` doesn't swallow the statements after it.
fn if_head_end(tokens: &[Token], start: usize) -> usize {
    let mut depth = 0i32;
    let mut j = start;
    while j < tokens.len() {
        match tokens[j].token_type {
            TokenType::LPAREN => depth += 1,
            TokenType::RPAREN => depth -= 1,
            TokenType::KwThen | TokenType::SEMI if depth == 0 => return j,
            _ => {}
        }
        j += 1;
    }
    j
}

fn cmp_op(t: TokenType) -> Option<Op> {
    Some(match t {
        TokenType::ASSIGN | TokenType::KwEQ => Op::Eq,
        TokenType::NE | TokenType::KwNE => Op::Ne,
        TokenType::LT | TokenType::KwLT => Op::Lt,
        TokenType::LE | TokenType::KwLE => Op::Le,
        TokenType::GT | TokenType::KwGT => Op::Gt,
        TokenType::GE | TokenType::KwGE => Op::Ge,
        _ => return None,
    })
}

fn is_literal(t: TokenType) -> bool {
    matches!(
        t,
        TokenType::IntegerLiteral
            | TokenType::FloatLiteral
            | TokenType::FloatExponentLiteral
            | TokenType::StringLiteral
    )
}

fn parse_lit(tok: &Token) -> Value {
    let txt = tok.text.as_str();
    match tok.token_type {
        TokenType::StringLiteral => {
            let quoted = txt.len() >= 2 && (txt.starts_with('"') || txt.starts_with('\''));
            Value::Str(if quoted {
                txt[1..txt.len() - 1].to_string()
            } else {
                txt.to_string()
            })
        }
        TokenType::IntegerLiteral => txt
            .parse::<i64>()
            .map(Value::Int)
            .unwrap_or_else(|_| Value::Float(txt.parse().unwrap_or(0.0))),
        _ => Value::Float(txt.parse().unwrap_or(0.0)),
    }
}

/// Recursive-descent parser for `if`-head predicates. Anything it can't
/// classify falls through to [`Pred::Tautology`].
struct PredicateParser<'a> {
    t: &'a [Token],
    i: usize,
    arrays: &'a HashMap<String, ArrayDecl>,
    used: &'a mut HashSet<String>,
}

impl<'a> PredicateParser<'a> {
    fn new(
        t: &'a [Token],
        arrays: &'a HashMap<String, ArrayDecl>,
        used: &'a mut HashSet<String>,
    ) -> Self {
        PredicateParser {
            t,
            i: 0,
            arrays,
            used,
        }
    }

    fn parse(&mut self) -> Pred {
        if self.t.is_empty() {
            return Pred::Tautology;
        }
        self.parse_or().unwrap_or(Pred::Tautology)
    }

    fn peek(&self, want: TokenType) -> bool {
        is(self.t, self.i, want)
    }

    fn peek_at(&self, idx: usize, want: TokenType) -> bool {
        is(self.t, idx, want)
    }

    fn parse_or(&mut self) -> Option<Pred> {
        let left = self.parse_and()?;
        let mut children = vec![left];
        while self.peek(TokenType::KwOR) {
            self.i += 1;
            if let Some(right) = self.parse_and() {
                children.push(right);
            }
        }
        Some(if children.len() == 1 {
            children.pop().unwrap()
        } else {
            Pred::Or(children)
        })
    }

    fn parse_and(&mut self) -> Option<Pred> {
        let left = self.parse_unary()?;
        let mut children = vec![left];
        while self.peek(TokenType::KwAND) {
            self.i += 1;
            if let Some(right) = self.parse_unary() {
                children.push(right);
            }
        }
        Some(if children.len() == 1 {
            children.pop().unwrap()
        } else {
            Pred::And(children)
        })
    }

    fn parse_unary(&mut self) -> Option<Pred> {
        if self.peek(TokenType::KwNOT) || self.peek(TokenType::NOT) {
            self.i += 1;
            return self.parse_unary().map(|p| Pred::Not(Box::new(p)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Option<Pred> {
        if self.peek(TokenType::LPAREN) {
            let start = self.i;
            self.i += 1;
            let inner = self.parse_or();
            if self.peek(TokenType::RPAREN) {
                self.i += 1;
                if inner.is_some() {
                    return inner;
                }
            }
            self.i = start;
            self.skip_paren();
            return Some(Pred::Tautology);
        }
        Some(self.parse_atom())
    }

    /// Atom shapes recognised:
    ///
    ///   IDENT IN ( lit, … )                membership
    ///   ARR[i] IN ( lit, … )               array-element membership
    ///   IDENT cmp_op LITERAL               equality / inequality / range
    ///   sum( ident, … ) cmp_op LITERAL     sum-threshold
    ///   wrapper( … ( IDENT ) … ) IN ( … )  upcase(trim(IDENT)) idiom
    fn parse_atom(&mut self) -> Pred {
        if self.i >= self.t.len() {
            return Pred::Tautology;
        }
        let ident = self.peek(TokenType::Identifier);
        let next_lparen = self.peek_at(self.i + 1, TokenType::LPAREN);

        if ident && next_lparen && self.t[self.i].text.eq_ignore_ascii_case("sum") {
            if let Some(p) = self.parse_sum() {
                return p;
            }
        }
        if ident
            && self.peek_at(self.i + 1, TokenType::LBRACK)
            && self.arrays.contains_key(&self.t[self.i].text)
        {
            if let Some(p) = self.parse_array_element_atom() {
                return p;
            }
        }
        if ident && next_lparen {
            if let Some(p) = self.parse_wrapped_in_atom() {
                return p;
            }
        }
        if ident {
            if let Some(p) = self.parse_basic_atom() {
                return p;
            }
        }
        self.skip_to_boundary();
        Pred::Tautology
    }

    fn parse_basic_atom(&mut self) -> Option<Pred> {
        let var = self.t[self.i].text.clone();
        let save = self.i;
        self.i += 1;

        if self.peek(TokenType::KwIN) && self.peek_at(self.i + 1, TokenType::LPAREN) {
            self.i += 2;
            let values = self.consume_literal_list_until_rparen();
            return Some(self.atom(var, Op::In, values));
        }

        if let Some(op) = ty(self.t, self.i).and_then(cmp_op) {
            self.i += 1;
            if let Some(v) = self.consume_literal() {
                return Some(self.atom(var, op, vec![v]));
            }
        }

        self.i = save;
        None
    }

    fn atom(&mut self, var: String, op: Op, values: Vec<Value>) -> Pred {
        self.used.insert(var.clone());
        let is_array = self.arrays.contains_key(&var);
        Pred::Atom(Atom {
            var,
            op,
            values,
            is_array,
        })
    }

    fn parse_sum(&mut self) -> Option<Pred> {
        self.i += 2; // sum (
        let mut args = Vec::new();
        let mut depth = 1;
        while self.i < self.t.len() {
            let t = &self.t[self.i];
            match t.token_type {
                TokenType::LPAREN => depth += 1,
                TokenType::RPAREN => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                TokenType::Identifier if depth == 1 => args.push(t.text.clone()),
                _ => {}
            }
            self.i += 1;
        }
        if !self.peek(TokenType::RPAREN) {
            return None;
        }
        self.i += 1;

        let op = ty(self.t, self.i).and_then(cmp_op)?;
        self.i += 1;
        let value = self.consume_literal().filter(Value::is_numeric)?;
        self.used.extend(args.iter().cloned());
        Some(Pred::Sum(SumAtom {
            vars: args,
            op,
            value,
        }))
    }

    /// `upcase(trim(IDENT))` style. Walk forward through nested `IDENT(`
    /// groups, capture the deepest identifier, then look for `in (` after
    /// the matching close-parens.
    fn parse_wrapped_in_atom(&mut self) -> Option<Pred> {
        let save = self.i;
        let mut depth = 0;
        while self.peek(TokenType::Identifier)
            && self.peek_at(self.i + 1, TokenType::LPAREN)
            && !self.t[self.i].text.eq_ignore_ascii_case("sum")
        {
            self.i += 2;
            depth += 1;
        }
        let ok = self.peek(TokenType::Identifier) && self.peek_at(self.i + 1, TokenType::RPAREN);
        if !ok {
            self.i = save;
            return None;
        }
        let inner = self.t[self.i].text.clone();
        self.i += 1;
        for _ in 0..depth {
            if !self.peek(TokenType::RPAREN) {
                self.i = save;
                return None;
            }
            self.i += 1;
        }
        if !(self.peek(TokenType::KwIN) && self.peek_at(self.i + 1, TokenType::LPAREN)) {
            self.i = save;
            return None;
        }
        self.i += 2;
        let values = self.consume_literal_list_until_rparen();
        Some(self.atom(inner, Op::In, values))
    }

    fn parse_array_element_atom(&mut self) -> Option<Pred> {
        let var = self.t[self.i].text.clone();
        let save = self.i;
        self.i += 2; // name [
        let mut depth = 1;
        while self.i < self.t.len() {
            match self.t[self.i].token_type {
                TokenType::LBRACK => depth += 1,
                TokenType::RBRACK => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            self.i += 1;
        }
        if !self.peek(TokenType::RBRACK) {
            return None;
        }
        self.i += 1;

        if !(self.peek(TokenType::KwIN) && self.peek_at(self.i + 1, TokenType::LPAREN)) {
            self.i = save;
            return None;
        }
        self.i += 2;
        let values = self.consume_literal_list_until_rparen();
        self.used.insert(var.clone());
        Some(Pred::Atom(Atom {
            var,
            op: Op::In,
            values,
            is_array: true,
        }))
    }

    fn consume_literal_list_until_rparen(&mut self) -> Vec<Value> {
        let mut values = Vec::new();
        let mut depth = 1;
        while self.i < self.t.len() {
            let t = &self.t[self.i];
            match t.token_type {
                TokenType::LPAREN => depth += 1,
                TokenType::RPAREN => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                tt if is_literal(tt) && depth == 1 => {
                    let v = parse_lit(t);
                    // `lo : hi` colon-range inside `in(…)` — expand to every
                    // integer in the inclusive span.
                    if let Value::Int(lo) = v {
                        if self.peek_at(self.i + 1, TokenType::COLON)
                            && ty(self.t, self.i + 2).is_some_and(is_literal)
                        {
                            if let Value::Int(hi) = parse_lit(&self.t[self.i + 2]) {
                                if hi >= lo {
                                    values.extend((lo..=hi).map(Value::Int));
                                    self.i += 3;
                                    continue;
                                }
                            }
                        }
                    }
                    values.push(v);
                }
                _ => {}
            }
            self.i += 1;
        }
        if self.peek(TokenType::RPAREN) {
            self.i += 1;
        }
        values
    }

    fn consume_literal(&mut self) -> Option<Value> {
        let t = self.t.get(self.i).filter(|t| is_literal(t.token_type))?;
        self.i += 1;
        Some(parse_lit(t))
    }

    fn skip_to_boundary(&mut self) {
        while let Some(t) = ty(self.t, self.i) {
            match t {
                TokenType::KwAND | TokenType::KwOR | TokenType::RPAREN => break,
                TokenType::LPAREN => self.skip_paren(),
                _ => self.i += 1,
            }
        }
    }

    fn skip_paren(&mut self) {
        if !self.peek(TokenType::LPAREN) {
            return;
        }
        let mut depth = 0;
        while let Some(t) = ty(self.t, self.i) {
            match t {
                TokenType::LPAREN => depth += 1,
                TokenType::RPAREN => {
                    depth -= 1;
                    if depth == 0 {
                        self.i += 1;
                        return;
                    }
                }
                _ => {}
            }
            self.i += 1;
        }
    }
}

// ── Char-input inference ────────────────────────────────────────────────
// Inputs whose only observed atom values are strings (or whose array decl
// was `$char`) get a "" default rather than 0.

fn infer_char_inputs(predicates: &[Pred]) -> HashSet<String> {
    let mut out = HashSet::new();
    walk_atoms(predicates, &mut |p| {
        if let Pred::Atom(a) = p {
            if !a.values.is_empty()
                && !a.is_array
                && a.values.iter().all(|v| matches!(v, Value::Str(_)))
            {
                out.insert(a.var.clone());
            }
        }
    });
    out
}

fn walk_atoms(predicates: &[Pred], f: &mut dyn FnMut(&Pred)) {
    for p in predicates {
        walk_atom(p, f);
    }
}

fn walk_atom(node: &Pred, f: &mut dyn FnMut(&Pred)) {
    match node {
        Pred::Atom(_) | Pred::Sum(_) => f(node),
        Pred::And(c) | Pred::Or(c) => c.iter().for_each(|p| walk_atom(p, f)),
        Pred::Not(c) => walk_atom(c, f),
        Pred::Tautology => {}
    }
}

// ── Per-input value sets ────────────────────────────────────────────────

fn build_value_sets(
    inputs: &[String],
    predicates: &[Pred],
    arrays: &HashMap<String, ArrayDecl>,
    specs: &ValueSpecs,
    char_inputs: &HashSet<String>,
) -> HashMap<String, Vec<Value>> {
    let mut sets: HashMap<String, Vec<Value>> = HashMap::new();

    walk_atoms(predicates, &mut |p| match p {
        Pred::Atom(a) => {
            let targets: Vec<String> = match arrays.get(&a.var).filter(|_| a.is_array) {
                Some(decl) => decl.members.clone(),
                None => vec![a.var.clone()],
            };
            // `gt`/`ge`/`lt`/`le` thresholds also seed the opposite side so
            // non-satisfying values are reachable in uniform sampling.
            let mut extra = Vec::new();
            if matches!(a.op, Op::Gt | Op::Ge | Op::Lt | Op::Le) {
                match a.values.first() {
                    Some(Value::Int(n)) => extra.extend(
                        [n - 1, n + 1]
                            .into_iter()
                            .filter(|x| *x >= 0)
                            .map(Value::Int),
                    ),
                    Some(Value::Float(x)) => extra.extend(
                        [x - 1.0, x + 1.0]
                            .into_iter()
                            .filter(|x| *x >= 0.0)
                            .map(Value::Float),
                    ),
                    _ => {}
                }
            }
            for t in targets {
                let set = sets.entry(t).or_default();
                set.extend(a.values.iter().cloned());
                set.extend(extra.iter().cloned());
            }
        }
        Pred::Sum(s) => {
            for v in &s.vars {
                sets.entry(v.clone())
                    .or_default()
                    .extend((0..=s.value.to_i64()).map(Value::Int));
            }
        }
        _ => {}
    });

    for var in inputs {
        let set = sets.entry(var.clone()).or_default();
        if let Some(spec) = specs.lookup(var) {
            set.extend(metadata_values(&spec.kind).into_iter().map(Value::Int));
        }
        set.push(if char_inputs.contains(var) {
            Value::Str(String::new())
        } else {
            Value::Int(0)
        });
    }

    for set in sets.values_mut() {
        let mut uniq: Vec<Value> = Vec::with_capacity(set.len());
        for v in set.drain(..) {
            if !uniq.contains(&v) {
                uniq.push(v);
            }
        }
        *set = uniq;
    }
    sets
}

/// Up to 9 representative values from a metadata spec — first, last, and
/// a handful of midpoints. Some specs span thousands of values (ages,
/// dollar amounts) so we don't enumerate fully.
fn metadata_values(kind: &SpecKind) -> Vec<i64> {
    match kind {
        SpecKind::Range(lo, hi) => {
            if hi < lo {
                return Vec::new();
            }
            let size = hi - lo + 1;
            if size <= 9 {
                return (*lo..=*hi).collect();
            }
            let mut out = vec![*lo];
            out.extend((1..=7).map(|k| lo + size * k / 8));
            out.push(*hi);
            out.dedup();
            out
        }
        SpecKind::Set(values) => values.clone(),
    }
}

fn build_default_row(
    inputs: &[String],
    value_sets: &HashMap<String, Vec<Value>>,
    char_inputs: &HashSet<String>,
) -> HashMap<String, Value> {
    inputs
        .iter()
        .map(|var| {
            let fallback = if char_inputs.contains(var) {
                Value::Str(String::new())
            } else {
                Value::Int(0)
            };
            let v = match value_sets.get(var) {
                None => fallback,
                Some(vs) if vs.is_empty() => fallback,
                Some(vs) if vs.iter().any(|v| v.as_f64() == Some(0.0)) => Value::Int(0),
                Some(vs)
                    if vs
                        .iter()
                        .any(|v| matches!(v, Value::Str(s) if s.is_empty())) =>
                {
                    Value::Str(String::new())
                }
                Some(vs) => vs[0].clone(),
            };
            (var.clone(), v)
        })
        .collect()
}

// ── Sampler ─────────────────────────────────────────────────────────────

type Row = HashMap<String, Value>;

fn sample_row(g: &Graph, mix: &Mix, rng: &mut Rng) -> Row {
    let mut strategy = pick_strategy(&mix.weights, rng);
    if matches!(strategy, Strategy::Single | Strategy::Combo) && g.predicates.is_empty() {
        strategy = Strategy::Baseline;
    }
    match strategy {
        Strategy::Baseline => g.default_row.clone(),
        Strategy::Single => {
            let pred = rng.choose(&g.predicates).unwrap();
            pinned_row(g, rng, &[pred])
        }
        Strategy::Combo => {
            let sizes = &mix.combo_sizes.0;
            let k = rng.range_usize(*sizes.start(), *sizes.end());
            let picks = rng.sample_k(&g.predicates, k);
            pinned_row(g, rng, &picks)
        }
        Strategy::Perturbation => {
            let mut row = g.default_row.clone();
            for var in &g.inputs {
                if rng.next_f64() < mix.perturbation_prob {
                    row.insert(var.clone(), sample_value_for(var, g, rng));
                }
            }
            row
        }
        Strategy::Chaos => g
            .inputs
            .iter()
            .map(|var| (var.clone(), sample_value_for(var, g, rng)))
            .collect(),
    }
}

fn pick_strategy(weights: &Weights, rng: &mut Rng) -> Strategy {
    let total: f64 = weights.0.iter().map(|(_, w)| w).sum();
    let r = rng.next_f64() * total;
    let mut acc = 0.0;
    for &(strategy, w) in &weights.0 {
        acc += w;
        if r < acc {
            return strategy;
        }
    }
    // Float round-off on `r ≈ total`: fall back to the last weighted one.
    weights
        .0
        .iter()
        .rev()
        .find(|(_, w)| *w > 0.0)
        .map_or(Strategy::Baseline, |p| p.0)
}

/// Satisfy each predicate in turn over the default row. Pins are
/// first-writer-wins per input, so two predicates with conflicting
/// constraints on the same variable (e.g. mutually exclusive outer
/// guards) don't corrupt the row — the later pick just drops cleanly.
fn pinned_row(g: &Graph, rng: &mut Rng, preds: &[&Pred]) -> Row {
    let mut pinned = Row::new();
    for p in preds {
        satisfy(p, &mut pinned, g, rng);
    }
    materialize(g, &pinned)
}

/// The default row with `pins` laid over it (non-input pins are dropped).
fn materialize(g: &Graph, pins: &Row) -> Row {
    let mut row = g.default_row.clone();
    for (k, v) in pins {
        if let Some(cell) = row.get_mut(k) {
            *cell = v.clone();
        }
    }
    row
}

fn sample_value_for(var: &str, g: &Graph, rng: &mut Rng) -> Value {
    match g.value_sets.get(var).and_then(|vs| rng.choose(vs)) {
        Some(v) => v.clone(),
        None => g.default_row.get(var).cloned().unwrap_or(Value::Missing),
    }
}

/// Recursively pin row cells to satisfy `node`. Best-effort; `Not` is a
/// no-op (the dominant idiom is `not(missing(X))`, which non-missing cell
/// defaults already satisfy).
fn satisfy(node: &Pred, row: &mut Row, g: &Graph, rng: &mut Rng) {
    match node {
        Pred::Atom(a) => satisfy_atom(a, row, g, rng),
        Pred::Sum(s) => satisfy_sum(s, row, g, rng),
        Pred::And(c) => c.iter().for_each(|p| satisfy(p, row, g, rng)),
        Pred::Or(c) => {
            if let Some(p) = rng.choose(c) {
                satisfy(p, row, g, rng);
            }
        }
        Pred::Not(_) | Pred::Tautology => {}
    }
}

fn satisfy_atom(atom: &Atom, row: &mut Row, g: &Graph, rng: &mut Rng) {
    let target = match g.arrays.get(&atom.var).filter(|_| atom.is_array) {
        Some(decl) => match rng.choose(&decl.members) {
            Some(m) => m.clone(),
            None => return,
        },
        None => atom.var.clone(),
    };
    if row.contains_key(&target) {
        return;
    }
    let v = pick_satisfying_value(atom, &target, g, rng);
    row.insert(target, v);
}

fn pick_satisfying_value(atom: &Atom, target: &str, g: &Graph, rng: &mut Rng) -> Value {
    let empty = Vec::new();
    let vs = g.value_sets.get(target).unwrap_or(&empty);
    let first = atom.values.first().cloned().unwrap_or(Value::Missing);
    match atom.op {
        Op::In | Op::Eq => {
            let pool = if atom.values.is_empty() {
                vs
            } else {
                &atom.values
            };
            rng.choose(pool).cloned().unwrap_or(Value::Missing)
        }
        Op::Ne => {
            let choices: Vec<&Value> = vs.iter().filter(|v| !atom.values.contains(v)).collect();
            match rng.choose(&choices) {
                Some(v) => (*v).clone(),
                None => outside(&atom.values, vs),
            }
        }
        Op::Gt => bump(first, 1),
        Op::Lt => bump(first, -1),
        Op::Ge | Op::Le => first,
    }
}

/// A value outside `excluded`, for `ne` / `not in` atoms whose value set
/// holds nothing else: one past the largest known number, or a string
/// none of the literals spell.
fn outside(excluded: &[Value], known: &[Value]) -> Value {
    if excluded.iter().any(|v| matches!(v, Value::Str(_))) {
        let mut s = String::from("Z");
        while excluded
            .iter()
            .any(|v| sas_cmp(v, &Value::Str(s.clone())) == Some(Ordering::Equal))
        {
            s.push('Z');
        }
        return Value::Str(s);
    }
    let max = excluded
        .iter()
        .chain(known)
        .filter_map(Value::as_f64)
        .fold(0.0, f64::max);
    Value::Int(max.floor() as i64 + 1)
}

fn bump(v: Value, delta: i64) -> Value {
    match v {
        Value::Int(n) => Value::Int(n + delta),
        Value::Float(x) => Value::Float(x + delta as f64 * 0.01),
        other => other,
    }
}

fn satisfy_sum(sum: &SumAtom, row: &mut Row, g: &Graph, rng: &mut Rng) {
    let n = sum.vars.len();
    if n == 0 {
        return;
    }
    let threshold = match sum.op {
        Op::Gt => sum.value.to_i64() + 1,
        Op::Le | Op::Lt => {
            for v in &sum.vars {
                let cell = row.entry(v.clone()).or_insert(Value::Missing);
                if *cell == Value::Missing {
                    *cell = Value::Int(0);
                }
            }
            return;
        }
        _ => sum.value.to_i64(),
    };

    let mut remaining = threshold;
    for (idx, v) in sum.vars.iter().enumerate() {
        if row.contains_key(v) {
            continue;
        }
        let share = if idx == n - 1 {
            remaining
        } else {
            let cap = max_value_for(v, g).unwrap_or(remaining);
            let up = remaining.min(cap);
            if up > 0 {
                rng.range_i64(0, up)
            } else {
                0
            }
        };
        let share = share.max(0);
        row.insert(v.clone(), Value::Int(share));
        remaining = (remaining - share).max(0);
    }
}

fn max_value_for(var: &str, g: &Graph) -> Option<i64> {
    g.value_sets
        .get(var)?
        .iter()
        .filter_map(Value::as_f64)
        .fold(None, |acc: Option<f64>, x| {
            Some(acc.map_or(x, |a| a.max(x)))
        })
        .map(|x| x.floor() as i64)
}

// ── Branch evaluation ───────────────────────────────────────────────────

/// What a row must satisfy to send `b` down `outcome`: the leaf (or its
/// negation) plus every reach condition, with `Not` pushed down to the
/// atoms so [`satisfy`] can pin it. The leaf comes first: pins are
/// first-writer-wins, and the leaf is usually the tighter bound on a
/// shared variable (`else if age lt 18` under `not (age ge 65)`).
fn requirement(b: &Branch, outcome: bool) -> Pred {
    let leaf = if outcome {
        positive(&b.leaf)
    } else {
        negate(&b.leaf)
    };
    Pred::And(
        [leaf]
            .into_iter()
            .chain(b.reach.iter().map(positive))
            .collect(),
    )
}

/// `node` with every `Not` eliminated.
fn positive(node: &Pred) -> Pred {
    match node {
        Pred::And(c) => Pred::And(c.iter().map(positive).collect()),
        Pred::Or(c) => Pred::Or(c.iter().map(positive).collect()),
        Pred::Not(p) => negate(p),
        other => other.clone(),
    }
}

/// The negation of `node`, in `Not`-free form. Unparsed conditions stay
/// [`Pred::Tautology`] and drop out of disjunctions so `satisfy` never
/// spends its OR pick on them.
fn negate(node: &Pred) -> Pred {
    match node {
        Pred::Atom(a) => Pred::Atom(Atom {
            op: match a.op {
                Op::In | Op::Eq => Op::Ne,
                Op::Ne => Op::In,
                Op::Lt => Op::Ge,
                Op::Le => Op::Gt,
                Op::Gt => Op::Le,
                Op::Ge => Op::Lt,
            },
            ..a.clone()
        }),
        Pred::Sum(s) => {
            let with = |op| Pred::Sum(SumAtom { op, ..s.clone() });
            match s.op {
                Op::In | Op::Eq => Pred::Or(vec![with(Op::Lt), with(Op::Gt)]),
                Op::Ne => with(Op::Eq),
                Op::Lt => with(Op::Ge),
                Op::Le => with(Op::Gt),
                Op::Gt => with(Op::Le),
                Op::Ge => with(Op::Lt),
            }
        }
        Pred::And(c) => {
            let mut alts: Vec<Pred> = c
                .iter()
                .map(negate)
                .filter(|p| !matches!(p, Pred::Tautology))
                .collect();
            match alts.len() {
                0 => Pred::Tautology,
                1 => alts.pop().unwrap(),
                _ => Pred::Or(alts),
            }
        }
        Pred::Or(c) => Pred::And(c.iter().map(negate).collect()),
        Pred::Not(p) => positive(p),
        Pred::Tautology => Pred::Tautology,
    }
}

/// True iff `row` arrives at `b` and takes `outcome`. Conditions over
/// arrays are checked per member index (one loop iteration each), and
/// any iteration counts. Unknowns are assumed to hold.
fn reaches(b: &Branch, outcome: bool, row: &Row, g: &Graph) -> bool {
    let mut slots = 0;
    walk_atoms(&b.reach, &mut |p| slots = slots.max(array_len(p, g)));
    walk_atom(&b.leaf, &mut |p| slots = slots.max(array_len(p, g)));
    let at = |slot: Option<usize>| {
        b.reach.iter().all(|p| eval(p, row, g, slot) != Some(false))
            && eval(&b.leaf, row, g, slot).unwrap_or(true) == outcome
    };
    if slots == 0 {
        at(None)
    } else {
        (0..slots).any(|k| at(Some(k)))
    }
}

fn array_len(node: &Pred, g: &Graph) -> usize {
    match node {
        Pred::Atom(a) if a.is_array => g.arrays.get(&a.var).map_or(0, |d| d.members.len()),
        _ => 0,
    }
}

/// Kleene three-valued evaluation of `node` against a full row. `None`
/// means unknown: an unparsed condition, a derived (non-input) variable,
/// or a comparison against non-literals. `slot` binds array atoms to one
/// member; `None` means "any member".
fn eval(node: &Pred, row: &Row, g: &Graph, slot: Option<usize>) -> Option<bool> {
    match node {
        Pred::Atom(a) => match g.arrays.get(&a.var).filter(|_| a.is_array) {
            Some(decl) => match slot {
                Some(k) => holds(a.op, row.get(decl.members.get(k)?)?, &a.values),
                None => kleene_or(
                    decl.members
                        .iter()
                        .map(|m| row.get(m).and_then(|v| holds(a.op, v, &a.values))),
                ),
            },
            None => holds(a.op, row.get(&a.var)?, &a.values),
        },
        Pred::Sum(s) => {
            // SAS `sum()` skips missing arguments.
            let mut total = 0.0;
            for v in &s.vars {
                match row.get(v)? {
                    Value::Missing => {}
                    x => total += x.as_f64()?,
                }
            }
            holds(s.op, &Value::Float(total), std::slice::from_ref(&s.value))
        }
        Pred::And(c) => kleene_and(c.iter().map(|p| eval(p, row, g, slot))),
        Pred::Or(c) => kleene_or(c.iter().map(|p| eval(p, row, g, slot))),
        Pred::Not(p) => eval(p, row, g, slot).map(|b| !b),
        Pred::Tautology => None,
    }
}

fn kleene_and(vals: impl Iterator<Item = Option<bool>>) -> Option<bool> {
    let mut unknown = false;
    for v in vals {
        match v {
            Some(false) => return Some(false),
            None => unknown = true,
            Some(true) => {}
        }
    }
    (!unknown).then_some(true)
}

fn kleene_or(vals: impl Iterator<Item = Option<bool>>) -> Option<bool> {
    kleene_and(vals.map(|v| v.map(|b| !b))).map(|b| !b)
}

fn holds(op: Op, v: &Value, values: &[Value]) -> Option<bool> {
    let first = values.first()?;
    let eq = || {
        values
            .iter()
            .any(|x| sas_cmp(v, x) == Some(Ordering::Equal))
    };
    let ord = sas_cmp(v, first);
    Some(match op {
        Op::In | Op::Eq => eq(),
        Op::Ne => !eq(),
        Op::Lt => ord == Some(Ordering::Less),
        Op::Le => matches!(ord, Some(Ordering::Less | Ordering::Equal)),
        Op::Gt => ord == Some(Ordering::Greater),
        Op::Ge => matches!(ord, Some(Ordering::Greater | Ordering::Equal)),
    })
}

/// SAS comparison: numeric missing sorts below every number, character
/// values compare with trailing blanks ignored (and missing as blank).
fn sas_cmp(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::Missing, Value::Missing) => Some(Ordering::Equal),
        (Value::Str(x), Value::Str(y)) => Some(x.trim_end().cmp(y.trim_end())),
        (Value::Str(x), Value::Missing) => Some(x.trim_end().cmp("")),
        (Value::Missing, Value::Str(y)) => Some("".cmp(y.trim_end())),
        (Value::Missing, _) => b.is_numeric().then_some(Ordering::Less),
        (_, Value::Missing) => a.is_numeric().then_some(Ordering::Greater),
        _ => a.as_f64()?.partial_cmp(&b.as_f64()?),
    }
}

// ── RNG ─────────────────────────────────────────────────────────────────

/// SplitMix64 — tiny, deterministic, and good enough for test-data
/// sampling. Avoids pulling in `rand` for one subcommand.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in `[0, n)`; `n` must be non-zero.
    fn below(&mut self, n: u64) -> u64 {
        ((self.next_u64() as u128 * n as u128) >> 64) as u64
    }

    fn range_i64(&mut self, lo: i64, hi: i64) -> i64 {
        lo + self.below((hi - lo) as u64 + 1) as i64
    }

    fn range_usize(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below((hi - lo) as u64 + 1) as usize
    }

    fn choose<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            return None;
        }
        Some(&items[self.below(items.len() as u64) as usize])
    }

    /// `k` distinct items (fewer if the slice is shorter), via a partial
    /// Fisher–Yates shuffle over indices.
    fn sample_k<'a, T>(&mut self, items: &'a [T], k: usize) -> Vec<&'a T> {
        let mut idx: Vec<usize> = (0..items.len()).collect();
        let k = k.min(items.len());
        for i in 0..k {
            let j = i + self.below((idx.len() - i) as u64) as usize;
            idx.swap(i, j);
        }
        idx[..k].iter().map(|&i| &items[i]).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(src: &str) -> Graph {
        build_graph(src, &ValueSpecs::default())
    }

    #[test]
    fn discovers_inputs_and_excludes_derived() {
        let g = graph(
            "data out; set in;\n\
             age = age;\n\
             if score ge 3 and flag = 1 then risk = 1;\n\
             if risk = 1 then tier = 2;\n\
             run;\n",
        );
        assert_eq!(g.inputs, vec!["age", "flag", "score"]);
    }

    #[test]
    fn expands_array_members_and_drops_array_name() {
        let g = graph(
            "array dx[3] $ dx1-dx3;\n\
             do i = 1 to 3; if dx[i] in ('A1', 'B2') then hit = 1; end;\n",
        );
        assert_eq!(g.inputs, vec!["dx1", "dx2", "dx3"]);
        assert_eq!(g.default_row["dx1"], Value::Str(String::new()));
    }

    #[test]
    fn nested_if_do_conjoins_outer_guard() {
        let g = graph("if x = 7 then do; if y in (1, 2) then z = 1; end;\n");
        assert_eq!(g.predicates.len(), 2);
        assert!(matches!(&g.predicates[1], Pred::And(c) if c.len() == 2));
    }

    #[test]
    fn subsetting_if_does_not_swallow_following_code() {
        let g = graph("if a = 1;\nif b = 2 then c = 1;\n");
        assert_eq!(g.inputs, vec!["a", "b"]);
        assert_eq!(g.predicates.len(), 2);
    }

    #[test]
    fn colon_ranges_expand_inside_in_lists() {
        let g = graph("if q in (1:4) then r = 1;\n");
        assert_eq!(
            g.value_sets["q"],
            vec![
                Value::Int(1),
                Value::Int(2),
                Value::Int(3),
                Value::Int(4),
                Value::Int(0)
            ]
        );
    }

    #[test]
    fn sampling_is_deterministic_and_sized() {
        let src = "if a = 1 or b gt 2 then c = 1;\nif sum(d1, d2) ge 3 then e = 1;\n";
        let specs = ValueSpecs::default();
        let one = sample(src, &specs, 50, 7);
        let two = sample(src, &specs, 50, 7);
        assert_eq!(one.rows.len(), 50);
        assert_eq!(one.rows, two.rows);
        assert_ne!(one.rows, sample(src, &specs, 50, 8).rows);
    }

    #[test]
    fn sum_rows_reach_threshold() {
        let g = graph("if sum(d1, d2, d3) ge 4 then e = 1;\n");
        let mut rng = Rng::new(1);
        for _ in 0..20 {
            let mut row = Row::new();
            satisfy(&g.predicates[0], &mut row, &g, &mut rng);
            let total: i64 = ["d1", "d2", "d3"].iter().map(|v| row[*v].to_i64()).sum();
            assert_eq!(total, 4);
        }
    }

    #[test]
    fn weights_parse_relative_and_reject_bad_specs() {
        let w: Weights = "single=3, combo=1".parse().unwrap();
        assert_eq!(
            w.to_string(),
            "baseline=0,single=3,combo=1,perturbation=0,chaos=0"
        );
        assert_eq!(
            Weights::default().to_string().parse::<Weights>(),
            Ok(Weights::default())
        );
        assert!("bogus=1"
            .parse::<Weights>()
            .unwrap_err()
            .contains("unknown strategy"));
        assert!("single=1,single=2".parse::<Weights>().is_err());
        assert!("single=-1".parse::<Weights>().is_err());
        assert!("chaos=0".parse::<Weights>().is_err());
        assert!("single".parse::<Weights>().is_err());
    }

    #[test]
    fn combo_sizes_parse_ranges_and_singletons() {
        assert_eq!("3-5".parse(), Ok(ComboSizes(3..=5)));
        assert_eq!("4".parse(), Ok(ComboSizes(4..=4)));
        assert!("5-3".parse::<ComboSizes>().is_err());
        assert!("0-2".parse::<ComboSizes>().is_err());
    }

    #[test]
    fn mix_weights_select_strategies() {
        let src = "if a = 1 then c = 1;\nif b in (2, 3) then d = 1;\n";
        let specs = ValueSpecs::default();
        let baseline = Mix {
            weights: "baseline=1".parse().unwrap(),
            ..Mix::default()
        };
        let out = sample_with(src, &specs, 20, 1, &baseline);
        assert!(out.rows.iter().all(|r| r == &out.rows[0]));
        let single = Mix {
            weights: "single=1".parse().unwrap(),
            ..Mix::default()
        };
        let out = sample_with(src, &specs, 20, 1, &single);
        assert!(out
            .rows
            .iter()
            .all(|r| r[0] == Value::Int(1) || r[1] != Value::Int(0)));
    }

    fn minimal_rows(src: &str) -> (Vec<Vec<String>>, Coverage) {
        let (out, cov) = minimal(src, &ValueSpecs::default(), 42);
        let rows = out
            .rows
            .iter()
            .map(|r| r.iter().map(Value::to_string).collect())
            .collect();
        (rows, cov)
    }

    #[test]
    fn minimal_covers_else_if_chains_in_source_order() {
        let (rows, cov) = minimal_rows(
            "if age ge 65 then do;\n\
               if sex = 'F' then g = 1;\n\
               else if sex = 'M' then g = 2;\n\
             end;\n\
             else if age lt 18 then g = 4;\n",
        );
        assert_eq!((cov.covered(), cov.total), (8, 8));
        // Three paths under `age ge 65` (F, M, neither), two beside it.
        assert_eq!(
            rows,
            vec![
                vec!["65", "F"],
                vec!["65", "M"],
                vec!["65", ""],
                vec!["17", ""],
                vec!["64", ""],
            ]
        );
    }

    #[test]
    fn minimal_finds_values_outside_every_excluded_list() {
        let (rows, cov) = minimal_rows("if a in (0) then x = 1;\nelse if a in (1) then x = 2;\n");
        assert!(cov.uncovered.is_empty());
        assert!(rows.contains(&vec!["2".to_string()]));
    }

    #[test]
    fn minimal_reports_infeasible_outcomes_and_honours_subsetting_if() {
        let (rows, cov) = minimal_rows(
            "if x = 1 then do;\n\
               if x = 2 then y = 1;\n\
             end;\n\
             if z gt 5;\n\
             if w = 3 then v = 1;\n",
        );
        assert_eq!(cov.uncovered, vec![(2, true)]);
        // Both `w` outcomes need `z gt 5` to get past the subsetting if.
        let w_rows: Vec<_> = rows.iter().filter(|r| r[0] == "3").collect();
        assert!(!w_rows.is_empty() && w_rows.iter().all(|r| r[2] == "6"));
    }

    #[test]
    fn minimal_covers_array_loops_and_sums() {
        let (rows, cov) = minimal_rows(
            "array dx[2] $ dx1-dx2;\n\
             do i = 1 to 2; if dx[i] in ('A1') then h = 1; end;\n\
             if sum(c1, c2) ge 3 then e = 1;\n",
        );
        assert!(cov.uncovered.is_empty(), "{:?}", cov.uncovered);
        assert!(rows.len() <= 2);
    }

    #[test]
    fn minimal_without_branches_emits_one_baseline_row() {
        let (rows, cov) = minimal_rows("x = x;\n");
        assert_eq!((rows, cov.total), (vec![vec!["0".to_string()]], 0));
    }

    #[test]
    fn output_stem_strips_sas_and_txt() {
        assert_eq!(
            output_stem(Path::new("a/B_1.0_2020-11-11.SAS")),
            "B_1.0_2020-11-11"
        );
        assert_eq!(output_stem(Path::new("x.txt")), "x");
        assert_eq!(output_stem(Path::new("x.csv")), "x.csv");
    }

    #[test]
    fn floats_keep_a_decimal_point() {
        assert_eq!(Value::Float(2.0).to_string(), "2.0");
        assert_eq!(Value::Float(0.75).to_string(), "0.75");
        assert_eq!(Value::Missing.to_string(), "");
    }
}
