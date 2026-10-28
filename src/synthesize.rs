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
use std::collections::{BTreeSet, HashMap, HashSet};
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
/// Equal slices a documented decimal range is cut into when seeding value
/// sets; the slice bounds (min and max included) are the values used.
const DECIMAL_SLICES: usize = 5;
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
    /// A whole number as [`Value::Int`], so it is written without a
    /// decimal point; anything else as [`Value::Float`].
    fn number(x: f64) -> Value {
        if x.fract() == 0.0 && x.abs() < 1e15 {
            Value::Int(x as i64)
        } else {
            Value::Float(x)
        }
    }

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

#[derive(Clone, Debug, PartialEq)]
struct Atom {
    var: String,
    op: Op,
    values: Vec<Value>,
    is_array: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct SumAtom {
    vars: Vec<String>,
    op: Op,
    value: Value,
}

/// `left op right` between two variables (`dob < ref_date`).
#[derive(Clone, Debug, PartialEq)]
struct CmpAtom {
    left: String,
    op: Op,
    right: String,
    /// `Some(k)` for `left - right op k`, which is missing when either is.
    diff: Option<f64>,
}

/// A counter `X = base; … if c then X = X + k; …` compared with literals:
/// `base` plus the `k` of each term whose condition holds.
#[derive(Clone, Debug, PartialEq)]
struct CountAtom {
    base: f64,
    terms: Vec<(Pred, f64)>,
    op: Op,
    values: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq)]
enum Pred {
    Atom(Atom),
    Sum(SumAtom),
    Cmp(CmpAtom),
    Count(CountAtom),
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

/// Derived variable `var` ending up with constant `value`, which some
/// assignment gives it, and the input condition `need` for that.
#[derive(Clone, Debug)]
struct Outcome {
    var: String,
    value: Value,
    need: Pred,
}

/// The source's logical graph — inputs, predicates, and per-input value sets.
#[derive(Debug)]
pub struct Graph {
    pub inputs: Vec<String>,
    predicates: Vec<Pred>,
    /// Decision points whose own condition reads an input; the targets
    /// [`minimal`] covers.
    branches: Vec<Branch>,
    /// Constants assigned to derived variables; the targets [`minimal`]
    /// covers alongside branches.
    outcomes: Vec<Outcome>,
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
    /// Catalog boundary values ([`boundary_values`]) placed in some row.
    pub boundaries: usize,
    /// Constants assigned to derived variables whose condition comes down
    /// to inputs, and of those, `(variable, value)` no row produces.
    pub values: usize,
    pub unproduced: Vec<(String, String)>,
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

/// Something a [`minimal`] row set must contain.
enum Target {
    /// Branch `.0` of [`Graph::branches`] taking outcome `.1`.
    Branch(usize, bool),
    /// [`Graph::outcomes`] `.0`: a derived variable given that constant.
    Value(usize),
    /// Input `.0` holding catalog boundary value `.1`.
    Boundary(String, Value),
}

/// The smallest row set found that drives every modelled `if` both ways
/// and puts each documented input at its catalog boundaries (see
/// [`boundary_values`]), ordered by the first branch outcome each row
/// reaches in source order.
///
/// Greedy set cover: branch outcomes are visited in source order, then
/// boundary values, and each one not already reached is packed into the
/// earliest row whose pins it doesn't disturb, else into a fresh row; rows
/// left redundant at the end are dropped. Unparsed conditions are assumed
/// to hold. Deterministic for a given seed.
pub fn minimal(source: &str, specs: &ValueSpecs, seed: u64) -> (Synthesized, Coverage) {
    struct Candidate {
        pins: Row,
        row: Row,
        /// Targets this row is responsible for; later merges must keep them.
        keep: Vec<usize>,
        /// Cells [`centre_ranges`] put mid-range; boundaries leave them be.
        centred: HashSet<String>,
    }

    let g = build_graph(source, specs);
    let mut rng = Rng::new(seed);
    let mut targets: Vec<Target> = (0..g.branches.len())
        .flat_map(|b| [Target::Branch(b, true), Target::Branch(b, false)])
        .chain((0..g.outcomes.len()).map(Target::Value))
        .collect();
    for var in &g.inputs {
        if let Some(spec) = specs.lookup(var) {
            for v in boundary_values(&spec.kind) {
                targets.push(Target::Boundary(var.clone(), v));
            }
        }
    }
    let hit = |row: &Row, t: usize| match &targets[t] {
        Target::Branch(b, outcome) => reaches(&g.branches[*b], *outcome, row, &g),
        Target::Value(o) => eval(&g.outcomes[*o].need, row, &g, None) == Some(true),
        Target::Boundary(var, v) => {
            row.get(var).and_then(|cell| sas_cmp(cell, v)) == Some(Ordering::Equal)
        }
    };

    // Pin target `t` into the earliest row that still reaches its own
    // targets, else a fresh row; it then becomes that row's to keep.
    let place = |t: usize, cands: &mut Vec<Candidate>, rng: &mut Rng| {
        let (need, reversed) = match &targets[t] {
            Target::Branch(..) | Target::Value(_) => {
                let need = match &targets[t] {
                    Target::Branch(b, outcome) => requirement(&g.branches[*b], *outcome),
                    Target::Value(o) => positive(&g.outcomes[*o].need),
                    Target::Boundary(..) => unreachable!(),
                };
                // Odd attempts pin outer conditions first, for when they're
                // the tighter bound.
                let mut reversed = need.clone();
                if let Pred::And(c) = &mut reversed {
                    c.reverse();
                }
                (Some(need), reversed)
            }
            Target::Boundary(..) => (None, Pred::Tautology),
        };
        let attempts = if need.is_some() { MINIMAL_ATTEMPTS } else { 1 };
        for ci in 0..=cands.len() {
            for attempt in 0..attempts {
                let base = cands.get(ci).map(|c| c.pins.clone()).unwrap_or_default();
                let mut pins = base.clone();
                let mut centred = cands.get(ci).map(|c| c.centred.clone()).unwrap_or_default();
                match (&targets[t], &need) {
                    (Target::Boundary(var, v), _) => {
                        // Boundaries may overwrite any cell the row's own
                        // targets don't depend on, except one deliberately
                        // put mid-range: an edge value there can break
                        // arithmetic the graph doesn't model.
                        if centred.contains(var) && pins.get(var) != Some(v) {
                            continue;
                        }
                        pins.insert(var.clone(), v.clone());
                    }
                    (_, Some(need)) => {
                        let need =
                            pick_disjuncts(if attempt % 2 == 0 { need } else { &reversed }, rng);
                        centred.extend(satisfy(&need, &mut pins, &g, rng));
                        repair(&need, &mut pins, &base, &g);
                    }
                    _ => unreachable!(),
                }
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
                        c.centred = centred;
                    }
                    None => cands.push(Candidate {
                        pins,
                        row,
                        keep: vec![t],
                        centred,
                    }),
                }
                return;
            }
        }
    };

    let mut cands: Vec<Candidate> = Vec::new();
    for (t, target) in targets.iter().enumerate() {
        if let Some(c) = cands.iter_mut().find(|c| hit(&c.row, t)) {
            // A boundary a row hits by accident stays free, so a later
            // boundary can still take that cell; the pass below restores
            // any that get overwritten.
            if !matches!(target, Target::Boundary(..)) {
                c.keep.push(t);
            }
            continue;
        }
        place(t, &mut cands, &mut rng);
    }
    let boundaries: Vec<usize> = (0..targets.len())
        .filter(|&t| matches!(targets[t], Target::Boundary(..)))
        .collect();
    let mut lost = Vec::new();
    for &t in &boundaries {
        match cands.iter_mut().find(|c| hit(&c.row, t)) {
            Some(c) => c.keep.push(t),
            None => lost.push(t),
        }
    }
    for t in lost {
        place(t, &mut cands, &mut rng);
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

    let placed = |t: &usize| order.iter().any(|&i| reached[i].contains(t));
    let mut total = 0;
    let mut boundaries = 0;
    let mut uncovered = Vec::new();
    let mut unproduced = Vec::new();
    for (t, target) in targets.iter().enumerate() {
        match target {
            Target::Branch(b, outcome) => {
                total += 1;
                if !placed(&t) {
                    uncovered.push((g.branches[*b].line, *outcome));
                }
            }
            Target::Value(o) => {
                if !placed(&t) {
                    let o = &g.outcomes[*o];
                    unproduced.push((o.var.clone(), o.value.to_string()));
                }
            }
            Target::Boundary(..) => boundaries += usize::from(placed(&t)),
        }
    }
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
            total,
            boundaries,
            values: g.outcomes.len(),
            unproduced,
            uncovered,
        },
    )
}

/// `node` with each OR collapsed to one randomly chosen disjunct, so the
/// result is a plain conjunction [`repair`] sees whole.
fn pick_disjuncts(node: &Pred, rng: &mut Rng) -> Pred {
    pick_within(node, &[], rng)
}

/// [`pick_disjuncts`] under the atoms the enclosing conjunctions fix,
/// passing over disjuncts that contradict them (the negated guard atoms
/// of `not (guard and x)`, when the row must pass the guard) unless every
/// disjunct does. Within a conjunction, the atoms each conjunct's pick
/// commits to bind the conjuncts after it, so counts over shared inputs
/// choose steps that agree.
fn pick_within(node: &Pred, fixed: &[Atom], rng: &mut Rng) -> Pred {
    match node {
        Pred::Or(c) => {
            let open: Vec<&Pred> = c.iter().filter(|p| !contradicts(p, fixed)).collect();
            let picked = if open.is_empty() {
                rng.choose(c)
            } else {
                rng.choose(&open).copied()
            };
            picked.map_or(Pred::Tautology, |p| pick_within(p, fixed, rng))
        }
        Pred::And(c) => {
            let merged = merge_counts(c);
            let mut fixed = fixed.to_vec();
            fixed.extend(merged.iter().filter_map(|p| match p {
                Pred::Atom(a) if !a.is_array => Some(a.clone()),
                _ => None,
            }));
            let mut picked = Vec::with_capacity(merged.len());
            for p in &merged {
                let pick = pick_within(p, &fixed, rng);
                if !matches!(p, Pred::Atom(_)) {
                    conjunct_atoms(&pick, &mut fixed);
                }
                picked.push(pick);
            }
            Pred::And(picked)
        }
        Pred::Count(c) => pick_within(&pick_count(c, fixed, rng), fixed, rng),
        other => other.clone(),
    }
}

/// `p` can't hold beside `fixed`: it is, or conjoins, an atom no value
/// satisfies together with the atoms `fixed` holds on its variable. The
/// candidates are missing, each literal those atoms compare with, and its
/// neighbours, which between them fall in every interval and list the
/// comparisons carve out.
fn contradicts(p: &Pred, fixed: &[Atom]) -> bool {
    match p {
        Pred::Atom(a) if !a.is_array => {
            let on: Vec<&Atom> = fixed
                .iter()
                .filter(|f| f.var == a.var && !f.is_array)
                .chain(std::iter::once(a))
                .collect();
            if on.len() == 1 {
                return false;
            }
            let mut pool = vec![Value::Missing];
            for v in on.iter().flat_map(|x| &x.values) {
                pool.extend([v.clone(), bump(v.clone(), -1), bump(v.clone(), 1)]);
            }
            !pool
                .iter()
                .any(|v| on.iter().all(|x| holds(x.op, v, &x.values) != Some(false)))
        }
        Pred::And(c) => c.iter().any(|p| contradicts(p, fixed)),
        _ => false,
    }
}

/// The atoms conjunction `p` commits to, appended to `out`.
fn conjunct_atoms(p: &Pred, out: &mut Vec<Atom>) {
    match p {
        Pred::Atom(a) if !a.is_array => out.push(a.clone()),
        Pred::And(c) => c.iter().for_each(|p| conjunct_atoms(p, out)),
        _ => {}
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

/// `tokens` with each `>` or `<` followed by an `=` token (`x > = 0`)
/// made the one `>=` or `<=` token SAS reads it as.
fn fuse_spaced_comparisons(tokens: Vec<Token>) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    for t in tokens {
        if t.token_type == TokenType::ASSIGN {
            if let Some(prev) = out.last_mut() {
                let fused = match prev.token_type {
                    TokenType::GT => Some((TokenType::GE, ">=")),
                    TokenType::LT => Some((TokenType::LE, "<=")),
                    _ => None,
                };
                if let Some((token_type, text)) = fused {
                    prev.token_type = token_type;
                    prev.text = text.to_string();
                    prev.end_line = t.end_line;
                    prev.end_column = t.end_column;
                    prev.end_byte = t.end_byte;
                    continue;
                }
            }
        }
        out.push(t);
    }
    out
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
    let tokens = fuse_spaced_comparisons(TokenStream::tokenize(source).default);
    let arrays = collect_array_decls(&tokens);
    let self_declared = collect_self_declared(&tokens);
    let assigned = collect_assigned(&tokens);
    let aliases = collect_aliases(&tokens);
    let mut walker = Walker {
        tokens: &tokens,
        arrays: &arrays,
        aliases: &aliases,
        used: HashSet::new(),
        preds: Vec::new(),
        branches: Vec::new(),
        defs: Vec::new(),
        computed: HashSet::new(),
    };
    // An `end;` left unmatched at the top level (a validity guard
    // commented out above its block) closes nothing: SAS is run with the
    // body balanced by an unconditional `do;`, so the walk carries on.
    let mut i = 0;
    while i < tokens.len() {
        i = walker.walk(i, &[], &[]) + 1;
    }
    let Walker {
        used,
        preds: mut predicates,
        mut branches,
        defs,
        computed,
        ..
    } = walker;

    // Rewrite conditions on derived constants into conditions on what
    // decides them, so guards like `flag in (0,1)` become reachable.
    let assigned_constants: Vec<(String, Value)> = defs
        .iter()
        .filter_map(|d| match &d.value {
            DefValue::Const(v) if *v != Value::Missing => Some((d.var.clone(), v.clone())),
            _ => None,
        })
        .collect();
    let mut derived = derived_definitions(defs, &computed, &self_declared);
    flatten_linear(&mut derived);
    linear_values(&mut derived, specs);
    for p in predicates.iter_mut() {
        *p = substitute_derived(p, &derived, DERIVED_DEPTH);
    }
    for b in branches.iter_mut() {
        b.leaf = substitute_derived(&b.leaf, &derived, DERIVED_DEPTH);
        for r in b.reach.iter_mut() {
            *r = substitute_derived(r, &derived, DERIVED_DEPTH);
        }
    }

    let mut base: HashSet<String> = self_declared;
    base.extend(used.difference(&assigned).cloned());
    base.extend(collect_read(&tokens).difference(&assigned).cloned());
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

    // Each constant a derived variable can be given, in source order,
    // where its condition comes down to inputs.
    let mut outcomes: Vec<Outcome> = Vec::new();
    for (var, value) in assigned_constants {
        if !derived.contains_key(&var) || outcomes.iter().any(|o| o.var == var && o.value == value)
        {
            continue;
        }
        let atom = Pred::Atom(Atom {
            var: var.clone(),
            op: Op::Eq,
            values: vec![value.clone()],
            is_array: false,
        });
        let need = substitute_within(
            &atom,
            &derived,
            DERIVED_DEPTH,
            &mut HashMap::new(),
            OUTCOME_BUDGET,
        );
        if need != atom && references_input(&need, &input_set, &arrays) {
            outcomes.push(Outcome { var, value, need });
        }
    }

    let mut char_inputs: HashSet<String> = HashSet::new();
    for decl in arrays.values().filter(|d| d.char) {
        char_inputs.extend(decl.members.iter().cloned());
    }
    char_inputs.extend(infer_char_inputs(&predicates));
    char_inputs.extend(
        inputs
            .iter()
            .filter(|v| matches!(specs.lookup(v).map(|s| &s.kind), Some(SpecKind::Date)))
            .cloned(),
    );

    let value_sets = build_value_sets(&inputs, &predicates, &arrays, specs, &char_inputs);
    let default_row = build_default_row(&inputs, &value_sets, &char_inputs);

    Graph {
        inputs,
        predicates,
        branches,
        outcomes,
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
        Pred::Cmp(c) => inputs.contains(c.left.as_str()) || inputs.contains(c.right.as_str()),
        Pred::Count(c) => c
            .terms
            .iter()
            .any(|(p, _)| references_input(p, inputs, arrays)),
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

/// How many layers of derived-on-derived definitions to unfold.
const DERIVED_DEPTH: usize = 4;

/// Constant definitions per derived variable, keeping only variables every
/// assignment of which is a constant. A declared input (`x = x;`) that the
/// source also assigns keeps its input value until an assignment fires,
/// so it starts with an unconditional [`DefValue::Input`].
fn derived_definitions(
    defs: Vec<Def>,
    computed: &HashSet<String>,
    self_declared: &HashSet<String>,
) -> HashMap<String, Vec<Def>> {
    let mut out: HashMap<String, Vec<Def>> = HashMap::new();
    for d in defs {
        if !computed.contains(&d.var) {
            out.entry(d.var.clone()).or_default().push(d);
        }
    }
    for (var, defs) in out.iter_mut() {
        if self_declared.contains(var) {
            defs.insert(
                0,
                Def {
                    var: var.clone(),
                    value: DefValue::Input,
                    cond: Vec::new(),
                },
            );
        }
    }
    out
}

/// The values each variable of a [`DefValue::Linear`] can take: those its
/// condition lists (`a in (0,1)`), its catalog spec's, or a derived
/// variable's ([`derived_values`]). A variable with a sum over anything
/// else is dropped from `derived`, so conditions on it stay unknowns.
fn linear_values(derived: &mut HashMap<String, Vec<Def>>, specs: &ValueSpecs) {
    let mut filled: HashMap<(String, usize), Vec<Vec<f64>>> = HashMap::new();
    let mut unmodelled = Vec::new();
    for (var, defs) in derived.iter() {
        for (i, d) in defs.iter().enumerate() {
            let (DefValue::Linear(_, terms) | DefValue::Sum(terms)) = &d.value else {
                continue;
            };
            let values: Option<Vec<Vec<f64>>> = terms
                .iter()
                .map(|t| match t.test {
                    Some(_) => Some(Vec::new()),
                    None if !t.max_of.is_empty() => {
                        let mut union: BTreeSet<i64> = BTreeSet::new();
                        for arg in &t.max_of {
                            let vs = term_values(arg, &d.cond, derived, specs, DERIVED_DEPTH)?;
                            union.extend(vs.into_iter().map(milli));
                        }
                        Some(union.into_iter().map(|t| t as f64 / 1000.0).collect())
                    }
                    None => term_values(&t.var, &d.cond, derived, specs, DERIVED_DEPTH),
                })
                .collect();
            // A `max(…)` term is only modelled by enumerating combinations.
            let skips = matches!(d.value, DefValue::Sum(_));
            let enumerable = |values: &[Vec<f64>]| {
                terms
                    .iter()
                    .zip(values)
                    .try_fold(1usize, |n, (t, v)| {
                        n.checked_mul(term_choices(t, v.len(), &d.cond, skips))
                    })
                    .is_some_and(|n| n <= LINEAR_COMBINATIONS)
            };
            let values =
                values.filter(|v| terms.iter().all(|t| t.max_of.is_empty()) || enumerable(v));
            match values {
                Some(v) => {
                    filled.insert((var.clone(), i), v);
                }
                None => unmodelled.push(var.clone()),
            }
        }
    }
    for ((var, i), values) in filled {
        if let Some(DefValue::Linear(_, terms) | DefValue::Sum(terms)) =
            derived.get_mut(&var).map(|defs| &mut defs[i].value)
        {
            for (t, v) in terms.iter_mut().zip(values) {
                t.values = v;
            }
        }
    }
    for var in unmodelled {
        derived.remove(&var);
    }
}

/// Inline into each [`DefValue::Linear`] the terms of a term variable that
/// is itself only ever a linear sum, assigned under the same conditions
/// (or fewer): `s2 = s1 + e;` after `s1 = a + b;` becomes `s2 = a + b + e`.
/// Sums of sums then stay one flat sum instead of rewriting each level
/// inside the next.
fn flatten_linear(derived: &mut HashMap<String, Vec<Def>>) {
    for _ in 0..DERIVED_DEPTH {
        let mut changed = false;
        let vars: Vec<String> = derived.keys().cloned().collect();
        for var in vars {
            for i in 0..derived[&var].len() {
                let DefValue::Linear(base, terms) = &derived[&var][i].value else {
                    continue;
                };
                let cond = &derived[&var][i].cond;
                let mut new_base = *base;
                let mut new_terms = Vec::with_capacity(terms.len());
                let mut inlined = false;
                for t in terms {
                    let inner = (t.test.is_none() && t.max_of.is_empty() && t.var != var)
                        .then(|| derived.get(&t.var))
                        .flatten()
                        .and_then(|defs| match &defs[..] {
                            [d] if cond.starts_with(&d.cond) => match &d.value {
                                DefValue::Linear(b, ts) => Some((*b, ts.clone())),
                                _ => None,
                            },
                            _ => None,
                        });
                    match inner {
                        Some((b, ts)) => {
                            new_base += b * t.coeff;
                            new_terms.extend(ts.into_iter().map(|mut u| {
                                u.coeff *= t.coeff;
                                u
                            }));
                            inlined = true;
                        }
                        None => new_terms.push(t.clone()),
                    }
                }
                if inlined {
                    derived.get_mut(&var).unwrap()[i].value = DefValue::Linear(new_base, new_terms);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

/// The values `var` can take where `cond` holds: those `cond` lists, its
/// catalog spec's, or a derived variable's ([`derived_values`]).
fn term_values(
    var: &str,
    cond: &[Pred],
    derived: &HashMap<String, Vec<Def>>,
    specs: &ValueSpecs,
    depth: usize,
) -> Option<Vec<f64>> {
    listed_values(var, cond)
        .or_else(|| match specs.lookup(var).map(|s| &s.kind) {
            Some(SpecKind::Set(v)) => Some(v.iter().map(|&x| x as f64).collect()),
            Some(SpecKind::Range(lo, hi)) if hi - lo < COUNTER_VALUES as i64 => {
                Some((*lo..=*hi).map(|x| x as f64).collect())
            }
            _ => None,
        })
        .or_else(|| derived_values(var, derived, specs, depth))
}

/// Every number derived variable `var` can be given: its constants, the
/// values of what it copies, the sums a [`DefValue::Linear`] of variables
/// with known values can make (`x = iD1 - 1`, `x = a + b`), or a counter's
/// totals. `None` when one isn't known or there are more than
/// [`COUNTER_VALUES`].
fn derived_values(
    var: &str,
    derived: &HashMap<String, Vec<Def>>,
    specs: &ValueSpecs,
    depth: usize,
) -> Option<Vec<f64>> {
    let defs = derived.get(var)?;
    if depth == 0 {
        return None;
    }
    if defs
        .iter()
        .any(|d| matches!(d.value, DefValue::Add(_) | DefValue::AddVar(..)))
    {
        return counter_values(var, derived);
    }
    let mut out: BTreeSet<i64> = BTreeSet::new();
    for d in defs {
        let values = match &d.value {
            DefValue::Const(Value::Missing) => Vec::new(),
            DefValue::Const(v) => vec![v.as_f64()?],
            DefValue::Copy(src) => term_values(src, &d.cond, derived, specs, depth - 1)?,
            DefValue::Linear(base, terms) => {
                let mut sums: BTreeSet<i64> = BTreeSet::from([milli(*base)]);
                for t in terms {
                    let steps = match t.test {
                        Some(_) => vec![0.0, 1.0],
                        None => term_values(&t.var, &d.cond, derived, specs, depth - 1)?,
                    };
                    sums = sums
                        .iter()
                        .flat_map(|s| steps.iter().map(move |v| s + milli(v * t.coeff)))
                        .collect();
                    if sums.len() > COUNTER_VALUES {
                        return None;
                    }
                }
                sums.into_iter().map(|t| t as f64 / 1000.0).collect()
            }
            _ => return None,
        };
        out.extend(values.into_iter().map(milli));
        if out.len() > COUNTER_VALUES {
            return None;
        }
    }
    Some(out.into_iter().map(|t| t as f64 / 1000.0).collect())
}

/// How many choices term `t`, with `n` values, offers a combination: a
/// test holds or not; a variable takes one of its values, or (in
/// `sum()`, where `cond` doesn't list its values) is missing; a `max(…)`
/// likewise, missing only when every argument may be.
fn term_choices(t: &LinearTerm, n: usize, cond: &[Pred], skips: bool) -> usize {
    match &t.test {
        Some(_) => 2,
        None if !t.max_of.is_empty() => {
            n + usize::from(skips && t.max_of.iter().all(|a| may_be_missing(a, cond)))
        }
        None => n + usize::from(skips && may_be_missing(&t.var, cond)),
    }
}

/// Whether `var` can be missing where `cond` holds: no condition lists
/// its values, or the list includes `.`.
fn may_be_missing(var: &str, cond: &[Pred]) -> bool {
    fn listed(var: &str, conds: &[Pred]) -> Option<bool> {
        conds.iter().find_map(|c| match c {
            Pred::Atom(a) if a.var == var && !a.is_array && matches!(a.op, Op::In | Op::Eq) => {
                Some(a.values.contains(&Value::Missing))
            }
            Pred::And(c) => listed(var, c),
            _ => None,
        })
    }
    listed(var, cond).unwrap_or(true)
}

/// Most combinations of term values a [`DefValue::Linear`] is rewritten
/// as one alternative each; beyond it the sum becomes a count.
const LINEAR_COMBINATIONS: usize = 128;

/// Largest rewrite of a comparison between variables with a derived side
/// ([`cmp_alternatives`]): one alternative per value of that side, each
/// carrying the conditions for that value.
const CMP_BUDGET: usize = 4 * DERIVED_BUDGET;

/// `c` as one alternative per value its derived side (the right one when
/// `right`) can be given, with the other side compared with a number:
/// `other - v op k` or `v - other op k`, and without a difference `other
/// op v` or `v op other`. A difference with a missing side is missing.
fn cmp_alternatives(
    c: &CmpAtom,
    right: bool,
    derived: &HashMap<String, Vec<Def>>,
) -> Option<Vec<Pred>> {
    let (side, other) = if right {
        (&c.right, &c.left)
    } else {
        (&c.left, &c.right)
    };
    let values = derived_values(side, derived, &ValueSpecs::default(), DERIVED_DEPTH)?;
    let atom = |var: &str, op: Op, v: Value| {
        Pred::Atom(Atom {
            var: var.to_string(),
            op,
            values: vec![v],
            is_array: false,
        })
    };
    let k = c.diff.unwrap_or(0.0);
    let mut alts: Vec<Pred> = values
        .iter()
        .map(|&v| {
            let compared = match (right, c.diff.is_some()) {
                (true, _) => atom(other, c.op, Value::number(v + k)),
                (false, true) => atom(other, flip(c.op), Value::number(v - k)),
                (false, false) => atom(other, flip(c.op), Value::number(v)),
            };
            Pred::And(vec![atom(side, Op::Eq, Value::number(v)), compared])
        })
        .collect();
    if c.diff.is_some() && holds(c.op, &Value::Missing, &[Value::number(k)]) == Some(true) {
        alts.push(atom(side, Op::Eq, Value::Missing));
        alts.push(atom(other, Op::Eq, Value::Missing));
    }
    Some(alts)
}

/// Largest rewritten condition (in predicate nodes) [`substitute_derived`]
/// will produce for one atom; beyond it the atom stays an unknown. Sources
/// that assign one variable many times under nested conditions (casemix
/// groupers) would otherwise unfold exponentially.
const DERIVED_BUDGET: usize = 1000;

fn pred_size(node: &Pred) -> usize {
    match node {
        Pred::And(c) | Pred::Or(c) => 1 + c.iter().map(pred_size).sum::<usize>(),
        Pred::Count(c) => 1 + c.terms.iter().map(|(p, _)| pred_size(p)).sum::<usize>(),
        Pred::Not(p) => 1 + pred_size(p),
        _ => 1,
    }
}

/// `node` with each atom on a derived constant `X op values` replaced by
/// the conditions under which `X` ends up with a value satisfying it: for
/// each definition `i` whose value qualifies, its condition and the
/// failure of every later definition (the last assignment wins); plus, if
/// missing qualifies, no definition firing at all. An atom nothing can
/// satisfy becomes an empty OR, which is false; one whose rewrite exceeds
/// [`DERIVED_BUDGET`] is left as it is.
fn substitute_derived(node: &Pred, derived: &HashMap<String, Vec<Def>>, depth: usize) -> Pred {
    substitute_cached(node, derived, depth, &mut HashMap::new())
}

/// Largest rewrite of the atom a value target (`x = 3`) asks for. Each
/// target is rewritten once, on its own, so it can be larger than the
/// conditions [`DERIVED_BUDGET`] bounds, which the sampler meets again and
/// again inside other conditions.
const OUTCOME_BUDGET: usize = 4 * DERIVED_BUDGET;

fn substitute_cached(
    node: &Pred,
    derived: &HashMap<String, Vec<Def>>,
    depth: usize,
    cache: &mut HashMap<String, Pred>,
) -> Pred {
    substitute_within(node, derived, depth, cache, DERIVED_BUDGET)
}

/// [`substitute_cached`] with `budget` for the atom `node` itself.
fn substitute_within(
    node: &Pred,
    derived: &HashMap<String, Vec<Def>>,
    depth: usize,
    cache: &mut HashMap<String, Pred>,
    budget: usize,
) -> Pred {
    if depth == 0 {
        return node.clone();
    }
    match node {
        // A comparison with a derived side: one alternative per value that
        // side can be given, the other side then compared with a number.
        Pred::Cmp(c) if derived.contains_key(&c.right) || derived.contains_key(&c.left) => {
            // Either derived side may be enumerated; the smaller rewrite
            // is kept.
            let out = [true, false]
                .into_iter()
                .filter(|&right| derived.contains_key(if right { &c.right } else { &c.left }))
                .filter_map(|right| {
                    let alts = cmp_alternatives(c, right, derived)?;
                    Some(substitute_cached(&Pred::Or(alts), derived, depth, cache))
                })
                .min_by_key(pred_size);
            match out {
                Some(out) if pred_size(&out) <= CMP_BUDGET => out,
                _ => node.clone(),
            }
        }
        Pred::Atom(a) if !a.is_array && derived.contains_key(&a.var) => {
            let key = format!("{}|{:?}|{:?}|{}", a.var, a.op, a.values, depth);
            if let Some(p) = cache.get(&key) {
                return p.clone();
            }
            let defs = &derived[&a.var];
            if defs
                .iter()
                .any(|d| matches!(d.value, DefValue::Add(_) | DefValue::AddVar(..)))
            {
                let out = substitute_counter(a, defs, derived, depth, cache)
                    .filter(|p| pred_size(p) <= budget)
                    .unwrap_or_else(|| node.clone());
                cache.insert(key, out.clone());
                return out;
            }
            let recur = |p: &Pred, cache: &mut HashMap<String, Pred>| {
                substitute_cached(p, derived, depth - 1, cache)
            };
            let mut not_fired: Vec<Option<Pred>> = Vec::with_capacity(defs.len());
            for d in defs {
                not_fired.push((!d.cond.is_empty()).then(|| {
                    Pred::Not(Box::new(Pred::And(
                        d.cond.iter().map(|p| recur(p, cache)).collect(),
                    )))
                }));
            }
            // None: an unconditional later assignment always wins.
            let none_after = |from: usize, i: Option<usize>| -> Option<Vec<Pred>> {
                (from..defs.len())
                    .filter(|&j| !i.is_some_and(|i| exclusive(defs, i, j)))
                    .map(|j| not_fired[j].clone())
                    .collect()
            };
            let mut alts = Vec::new();
            for (i, d) in defs.iter().enumerate() {
                // What the assigned value must satisfy, beyond reaching it.
                let value_ok = match &d.value {
                    DefValue::Const(v) if holds(a.op, v, &a.values) == Some(true) => None,
                    DefValue::Const(_) => continue,
                    // The input itself must satisfy the atom; it isn't
                    // substituted again.
                    DefValue::Input => Some(Pred::Atom(a.clone())),
                    DefValue::Copy(src) => Some(recur(
                        &Pred::Atom(Atom {
                            var: src.clone(),
                            ..a.clone()
                        }),
                        cache,
                    )),
                    // X = Y * k, so `X op v` is `Y op v/k`, turned round for k < 0.
                    DefValue::Add(_) | DefValue::AddVar(..) => {
                        unreachable!("counters are substituted above")
                    }
                    // The constant plus one count term per value of each
                    // variable, with every variable at one of its values;
                    // or, when the atom accepts missing, any one missing.
                    // `sum()` lets each variable be missing too, as long
                    // as one isn't, and is missing when all are.
                    DefValue::Linear(..) | DefValue::Sum(_) => {
                        let (base, terms, skips) = match &d.value {
                            DefValue::Linear(base, terms) => (*base, terms, false),
                            DefValue::Sum(terms) => (0.0, terms, true),
                            _ => unreachable!(),
                        };
                        let is = |var: &str, op: Op, values: Vec<Value>| {
                            Pred::Atom(Atom {
                                var: var.to_string(),
                                op,
                                values,
                                is_array: false,
                            })
                        };
                        let numbers = |vs: &[f64]| {
                            let mut out: Vec<Value> =
                                vs.iter().map(|&v| Value::number(v)).collect();
                            if skips {
                                out.push(Value::Missing);
                            }
                            out
                        };
                        let vars = || {
                            terms
                                .iter()
                                .filter(|t| t.test.is_none() && t.max_of.is_empty())
                        };
                        // Few enough combinations: one alternative per
                        // combination of term values the atom accepts,
                        // each pinning every term, so picks of one term
                        // can't disagree with the total. A choice adds a
                        // number, or None for a missing `sum()` argument;
                        // `sum()` is missing when every argument is.
                        let may_be_missing = |var: &str| skips && may_be_missing(var, &d.cond);
                        let choices: Vec<Vec<(Pred, Option<f64>)>> = terms
                            .iter()
                            .map(|t| {
                                let mut out = Vec::new();
                                if let Some(test) = &t.test {
                                    out.push((test.clone(), Some(t.coeff)));
                                    out.push((negate(test), Some(0.0)));
                                } else if !t.max_of.is_empty() {
                                    // `max(…) = v`: some argument is v and
                                    // none is more.
                                    for &v in &t.values {
                                        let at = |op| {
                                            t.max_of
                                                .iter()
                                                .map(|x| is(x, op, vec![Value::number(v)]))
                                                .collect()
                                        };
                                        out.push((
                                            Pred::And(vec![
                                                Pred::Or(at(Op::Eq)),
                                                Pred::And(at(Op::Le)),
                                            ]),
                                            Some(v * t.coeff),
                                        ));
                                    }
                                    if t.max_of.iter().all(|x| may_be_missing(x)) {
                                        let none = t
                                            .max_of
                                            .iter()
                                            .map(|x| is(x, Op::Eq, vec![Value::Missing]))
                                            .collect();
                                        out.push((Pred::And(none), None));
                                    }
                                } else {
                                    for &v in &t.values {
                                        out.push((
                                            is(&t.var, Op::Eq, vec![Value::number(v)]),
                                            Some(v * t.coeff),
                                        ));
                                    }
                                    if may_be_missing(&t.var) {
                                        out.push((is(&t.var, Op::Eq, vec![Value::Missing]), None));
                                    }
                                }
                                out
                            })
                            .collect();
                        let combos = choices
                            .iter()
                            .try_fold(1usize, |n, c| n.checked_mul(c.len()))
                            .filter(|&n| n > 0 && n <= LINEAR_COMBINATIONS);
                        let enumerated = combos.is_some().then(|| {
                            // A tree choosing each term's value in turn,
                            // keeping the branches some accepted total
                            // completes, so shared prefixes are written
                            // once.
                            fn branch(
                                choices: &[Vec<(Pred, Option<f64>)>],
                                total: Option<f64>,
                                accepts: &dyn Fn(Option<f64>) -> bool,
                            ) -> Option<Vec<Pred>> {
                                let Some((first, rest)) = choices.split_first() else {
                                    return accepts(total).then(Vec::new);
                                };
                                let alts: Vec<Pred> = first
                                    .iter()
                                    .filter_map(|(p, k)| {
                                        let total = match (total, k) {
                                            (Some(t), Some(k)) => Some(t + k),
                                            (None, k) => *k,
                                            (t, None) => t,
                                        };
                                        let mut all = vec![p.clone()];
                                        all.extend(branch(rest, total, accepts)?);
                                        Some(Pred::And(all))
                                    })
                                    .collect();
                                match alts.len() {
                                    0 => None,
                                    1 => match alts.into_iter().next() {
                                        Some(Pred::And(all)) => Some(all),
                                        _ => unreachable!(),
                                    },
                                    _ => Some(vec![Pred::Or(alts)]),
                                }
                            }
                            let accepts = |total: Option<f64>| {
                                let total =
                                    total.map_or(Value::Missing, |t| Value::number(base + t));
                                holds(a.op, &total, &a.values) == Some(true)
                            };
                            let choices: Vec<Vec<(Pred, Option<f64>)>> = choices
                                .iter()
                                .map(|c| c.iter().map(|(p, k)| (recur(p, cache), *k)).collect())
                                .collect();
                            match branch(&choices, None, &accepts) {
                                Some(all) => vec![Pred::And(all)],
                                None => Vec::new(),
                            }
                        });
                        let mut all: Vec<Pred> = vars()
                            .map(|t| recur(&is(&t.var, Op::In, numbers(&t.values)), cache))
                            .collect();
                        if skips {
                            all.push(Pred::Or(
                                vars()
                                    .map(|t| {
                                        recur(&is(&t.var, Op::Ne, vec![Value::Missing]), cache)
                                    })
                                    .collect(),
                            ));
                        }
                        let tests: Vec<(Pred, f64)> = terms
                            .iter()
                            .filter_map(|t| Some((recur(t.test.as_ref()?, cache), t.coeff)))
                            .collect();
                        let steps = vars()
                            .flat_map(|t| {
                                t.values.iter().filter(|&&v| v * t.coeff != 0.0).map(|&v| {
                                    (is(&t.var, Op::Eq, vec![Value::number(v)]), v * t.coeff)
                                })
                            })
                            .map(|(p, k)| (recur(&p, cache), k))
                            .chain(tests)
                            .collect();
                        all.push(Pred::Count(CountAtom {
                            base,
                            terms: steps,
                            op: a.op,
                            values: a.values.clone(),
                        }));
                        // An enumerated `sum()` already has its all-missing
                        // combinations.
                        // Large enumerations of plain sums stay counts,
                        // which grow with the terms rather than with
                        // their combinations.
                        let has_max = terms.iter().any(|t| !t.max_of.is_empty());
                        let enumerated = enumerated.filter(|alts| {
                            has_max
                                || alts.iter().map(pred_size).sum::<usize>() <= DERIVED_BUDGET / 2
                        });
                        let enumerated_sum = skips && enumerated.is_some();
                        let mut any_of = match enumerated {
                            Some(alternatives) => alternatives,
                            None => vec![Pred::And(all)],
                        };
                        if !enumerated_sum && holds(a.op, &Value::Missing, &a.values) == Some(true)
                        {
                            let missing = vars()
                                .map(|t| recur(&is(&t.var, Op::Eq, vec![Value::Missing]), cache));
                            if skips {
                                any_of.push(Pred::And(missing.collect()));
                            } else {
                                any_of.extend(missing);
                            }
                        }
                        Some(Pred::Or(any_of))
                    }
                    DefValue::Scaled(src, k) => {
                        let values: Option<Vec<Value>> = a
                            .values
                            .iter()
                            .map(|v| match v {
                                Value::Missing => Some(Value::Missing),
                                v => v.as_f64().map(|x| Value::Float(x / k)),
                            })
                            .collect();
                        let Some(values) = values else { continue };
                        let op = if *k < 0.0 { flip(a.op) } else { a.op };
                        Some(recur(
                            &Pred::Atom(Atom {
                                var: src.clone(),
                                op,
                                values,
                                ..a.clone()
                            }),
                            cache,
                        ))
                    }
                };
                if let Some(mut later) = none_after(i + 1, Some(i)) {
                    let mut all: Vec<Pred> = d.cond.iter().map(|p| recur(p, cache)).collect();
                    all.extend(value_ok);
                    all.append(&mut later);
                    // An empty AND (always reached) is true.
                    alts.push(Pred::And(all));
                }
            }
            if holds(a.op, &Value::Missing, &a.values) == Some(true) {
                if let Some(none) = none_after(0, None) {
                    alts.push(Pred::And(none));
                }
            }
            // No alternative: every assignment is modelled and none (nor
            // missing) satisfies the atom, so it is false. An empty OR is.
            let out = match alts.len() {
                1 => alts.pop().unwrap(),
                _ => Pred::Or(alts),
            };
            let out = if pred_size(&out) > budget {
                node.clone()
            } else {
                out
            };
            cache.insert(key, out.clone());
            out
        }
        Pred::And(c) => Pred::And(
            c.iter()
                .map(|p| substitute_cached(p, derived, depth, cache))
                .collect(),
        ),
        Pred::Or(c) => Pred::Or(
            c.iter()
                .map(|p| substitute_cached(p, derived, depth, cache))
                .collect(),
        ),
        Pred::Not(p) => Pred::Not(Box::new(substitute_cached(p, derived, depth, cache))),
        other => other.clone(),
    }
}

/// Definitions `i` and `j` can't both fire: one is reached only through
/// a condition the other is reached only through the `else` of, at any
/// depth of nested `if … then do;` blocks, or they need one variable in
/// lists with no value in common (`a = 0` and `a in (1,2)`).
fn exclusive(defs: &[Def], i: usize, j: usize) -> bool {
    let negates =
        |a: &[Pred], b: &[Pred]| b.iter().any(|c| matches!(c, Pred::Not(p) if a.contains(p)));
    let (li, lj) = (listed_atoms(&defs[i].cond), listed_atoms(&defs[j].cond));
    let disjoint = li.iter().any(|a| {
        lj.iter().any(|b| {
            a.var == b.var
                && a.values
                    .iter()
                    .all(|v| holds(Op::In, v, &b.values) == Some(false))
        })
    });
    disjoint || negates(&defs[i].cond, &defs[j].cond) || negates(&defs[j].cond, &defs[i].cond)
}

/// The `x in (…)` and `x = k` atoms among `conds` and their conjunctions.
fn listed_atoms(conds: &[Pred]) -> Vec<&Atom> {
    let mut out = Vec::new();
    for c in conds {
        match c {
            Pred::Atom(a) if !a.is_array && matches!(a.op, Op::In | Op::Eq) => out.push(a),
            Pred::And(c) => out.extend(listed_atoms(c)),
            _ => {}
        }
    }
    out
}

/// [`substitute_derived`] for an atom on a counter (`X = 0; … if c then
/// X = X + 1;`): for each constant reset that is the last to fire, its
/// value plus the step of every later increment that fires must satisfy
/// the atom. A missing reset stays missing, as `. + 1` is in SAS. `None`
/// when a reset isn't a numeric constant.
fn substitute_counter(
    a: &Atom,
    defs: &[Def],
    derived: &HashMap<String, Vec<Def>>,
    depth: usize,
    cache: &mut HashMap<String, Pred>,
) -> Option<Pred> {
    let mut recur = |conds: &[Pred]| -> Vec<Pred> {
        conds
            .iter()
            .map(|p| substitute_cached(p, derived, depth - 1, cache))
            .collect()
    };
    let resets: Vec<usize> = (0..defs.len())
        .filter(|&i| !matches!(defs[i].value, DefValue::Add(_) | DefValue::AddVar(..)))
        .collect();
    let mut alts = Vec::new();
    'reset: for &i in &resets {
        let DefValue::Const(base) = &defs[i].value else {
            return None;
        };
        let mut all = recur(&defs[i].cond);
        for &j in resets.iter().filter(|&&j| j > i && !exclusive(defs, i, j)) {
            if defs[j].cond.is_empty() {
                continue 'reset;
            }
            let fired = recur(&defs[j].cond);
            all.push(Pred::Not(Box::new(Pred::And(fired))));
        }
        if *base == Value::Missing {
            if holds(a.op, base, &a.values) != Some(true) {
                continue;
            }
        } else {
            let mut base = base.as_f64()?;
            let mut terms = Vec::new();
            for d in &defs[i + 1..] {
                // The reset fired, so the guards it shares with the step hold.
                let own = d.cond.strip_prefix(&defs[i].cond[..]).unwrap_or(&d.cond);
                match &d.value {
                    DefValue::Add(k) if own.is_empty() => base += k,
                    DefValue::Add(k) => terms.push((Pred::And(recur(own)), *k)),
                    DefValue::AddVar(y, offset) => {
                        // One term per value `y` can take. A missing `y`
                        // would leave the counter missing; requiring it
                        // present outright (not just when the step runs)
                        // keeps the requirement a plain conjunction. A
                        // derived `y` is rewritten like any condition.
                        let is = |v: Value| {
                            Pred::Atom(Atom {
                                var: y.clone(),
                                op: Op::Eq,
                                values: vec![v],
                                is_array: false,
                            })
                        };
                        all.extend(recur(&[negate(&is(Value::Missing))]));
                        let values =
                            listed_values(y, own).or_else(|| counter_values(y, derived))?;
                        for v in values {
                            if v + offset != 0.0 {
                                let mut when = own.to_vec();
                                when.push(is(Value::number(v)));
                                terms.push((Pred::And(recur(&when)), v + offset));
                            }
                        }
                    }
                    _ => {}
                }
            }
            all.push(Pred::Count(CountAtom {
                base,
                terms,
                op: a.op,
                values: a.values.clone(),
            }));
        }
        alts.push(Pred::And(all));
    }
    if holds(a.op, &Value::Missing, &a.values) == Some(true)
        && resets.iter().all(|&i| !defs[i].cond.is_empty())
    {
        let none = resets
            .iter()
            .map(|&i| Pred::Not(Box::new(Pred::And(recur(&defs[i].cond)))))
            .collect();
        alts.push(Pred::And(none));
    }
    match alts.len() {
        0 => None,
        1 => alts.pop(),
        _ => Some(Pred::Or(alts)),
    }
}

/// Most distinct values [`counter_values`] lists for one variable.
const COUNTER_VALUES: usize = 64;

/// Every numeric value derived variable `var` can end up with: its
/// constants, or for a counter each reset plus any combination of its
/// constant steps. `None` when that isn't a short finite list.
fn counter_values(var: &str, derived: &HashMap<String, Vec<Def>>) -> Option<Vec<f64>> {
    let defs = derived.get(var)?;
    let mut out: BTreeSet<i64> = BTreeSet::new();
    let milli = |x: f64| (x * 1000.0).round() as i64;
    let mut totals: BTreeSet<i64> = BTreeSet::new();
    for d in defs {
        match &d.value {
            DefValue::Const(Value::Missing) => {}
            DefValue::Const(v) => {
                out.append(&mut totals);
                totals.insert(milli(v.as_f64()?));
            }
            DefValue::Add(k) => {
                let stepped: Vec<i64> = totals.iter().map(|t| t + milli(*k)).collect();
                totals.extend(stepped);
            }
            _ => return None,
        }
        if totals.len() + out.len() > COUNTER_VALUES {
            return None;
        }
    }
    out.append(&mut totals);
    Some(out.into_iter().map(|t| t as f64 / 1000.0).collect())
}

/// The numbers a condition among `conds` limits `var` to (`var in (0,1,2)`
/// or `var = 3`), leaving out missing: the values a step by `var` can add
/// when it runs.
fn listed_values(var: &str, conds: &[Pred]) -> Option<Vec<f64>> {
    conds.iter().find_map(|c| match c {
        Pred::Atom(a) if a.var == var && !a.is_array && matches!(a.op, Op::In | Op::Eq) => a
            .values
            .iter()
            .filter(|v| **v != Value::Missing)
            .map(Value::as_f64)
            .collect(),
        Pred::And(c) => listed_values(var, c),
        _ => None,
    })
}

/// `c` as a plain conjunction: a random choice of terms whose steps bring
/// the base to a total the comparison accepts, the others negated.
/// Steps `fixed` rules in or out are taken or skipped as it says, when
/// some total is still accepted that way. [`Pred::Tautology`] when no
/// choice does.
fn pick_count(c: &CountAtom, fixed: &[Atom], rng: &mut Rng) -> Pred {
    let take: Vec<bool> = c
        .terms
        .iter()
        .map(|(p, _)| !contradicts(&positive(p), fixed))
        .collect();
    let skip: Vec<bool> = c
        .terms
        .iter()
        .map(|(p, _)| !contradicts(&negate(step_condition(p)), fixed))
        .collect();
    let bound = count_reachable_within(c, &take, &skip);
    let (reachable, take, skip) = if bound.last().unwrap().iter().any(|&t| count_accepts(c, t)) {
        (bound, take, skip)
    } else {
        let all = vec![true; c.terms.len()];
        (count_reachable(c), all.clone(), all)
    };
    let goals: Vec<i64> = reachable
        .last()
        .unwrap()
        .iter()
        .copied()
        .filter(|&t| count_accepts(c, t))
        .collect();
    if goals.is_empty() {
        return Pred::Tautology;
    }
    // The table above treats steps as independent, but steps on one input
    // (`a = 1`, `a = 3`) exclude each other, so a total can be reachable
    // only by picks that contradict. Walk to a total keeping the picks
    // consistent, and try again when that walk runs out of choices.
    let mut last = None;
    for _ in 0..COUNT_PICK_TRIES {
        let goal = *rng.choose(&goals).unwrap();
        let (picks, consistent) = walk_count(c, &reachable, &take, &skip, goal, fixed, rng);
        if consistent {
            return picks;
        }
        last = Some(picks);
    }
    last.unwrap()
}

/// Tries [`pick_count`] makes at a consistent choice of steps.
const COUNT_PICK_TRIES: usize = 8;

/// Steps of `c` reaching `goal`, chosen last to first: each taken or
/// skipped as `reachable` allows, preferring choices the picks so far
/// (and `fixed`) leave open. Also whether every choice could be.
fn walk_count(
    c: &CountAtom,
    reachable: &[BTreeSet<i64>],
    take: &[bool],
    skip: &[bool],
    goal: i64,
    fixed: &[Atom],
    rng: &mut Rng,
) -> (Pred, bool) {
    let mut need = goal;
    let mut picks = Vec::with_capacity(c.terms.len());
    let mut bound = fixed.to_vec();
    let mut consistent = true;
    for (j, (p, k)) in c.terms.iter().enumerate().rev() {
        let taken = positive(p);
        let skipped = negate(step_condition(p));
        let mut take = take[j] && reachable[j].contains(&(need - milli(*k)));
        let mut skip = skip[j] && reachable[j].contains(&need);
        let take_open = take && !contradicts(&taken, &bound);
        let skip_open = skip && !contradicts(&skipped, &bound);
        if take_open || skip_open {
            (take, skip) = (take_open, skip_open);
        } else {
            consistent = false;
        }
        let pick = if take && (!skip || rng.below(2) == 0) {
            need -= milli(*k);
            taken
        } else {
            skipped
        };
        conjunct_atoms(&pick, &mut bound);
        picks.push(pick);
    }
    picks.reverse();
    (Pred::And(picks), consistent)
}

/// Totals in thousandths, so sums of decimal steps compare exactly.
fn milli(x: f64) -> i64 {
    (x * 1000.0).round() as i64
}

/// The totals `c` can reach after each prefix of its terms, in
/// thousandths: element `j` holds those of the first `j` terms.
fn count_reachable(c: &CountAtom) -> Vec<BTreeSet<i64>> {
    let all = vec![true; c.terms.len()];
    count_reachable_within(c, &all, &all)
}

/// [`count_reachable`] when term `j` may be taken only if `take[j]` and
/// skipped only if `skip[j]`.
fn count_reachable_within(c: &CountAtom, take: &[bool], skip: &[bool]) -> Vec<BTreeSet<i64>> {
    let mut reachable = vec![BTreeSet::from([milli(c.base)])];
    for (j, (_, k)) in c.terms.iter().enumerate() {
        let prev = reachable.last().unwrap();
        let mut next = BTreeSet::new();
        for &t in prev {
            if skip[j] {
                next.insert(t);
            }
            if take[j] {
                next.insert(t + milli(*k));
            }
        }
        reachable.push(next);
    }
    reachable
}

fn count_accepts(c: &CountAtom, total: i64) -> bool {
    holds(c.op, &Value::Float(total as f64 / 1000.0), &c.values) == Some(true)
}

/// Conjuncts `c`, flattened, with comparisons on the same count (same
/// base and terms) merged into one `in` list of the totals all of them
/// accept, so a single pick of terms satisfies every one: `n ne 1` with
/// `n lt 2` becomes `n in (0)`. Picked separately, each would choose its
/// own total and the first to pin the shared cells would win.
fn merge_counts(c: &[Pred]) -> Vec<Pred> {
    fn flatten(p: &Pred, out: &mut Vec<Pred>) {
        match p {
            Pred::And(c) => c.iter().for_each(|p| flatten(p, out)),
            p => out.push(p.clone()),
        }
    }
    let mut flat = Vec::new();
    c.iter().for_each(|p| flatten(p, &mut flat));
    let mut out: Vec<Pred> = Vec::with_capacity(flat.len());
    for p in flat {
        let Pred::Count(n) = &p else {
            out.push(p);
            continue;
        };
        let same = out.iter_mut().find_map(|q| match q {
            Pred::Count(m) if m.base == n.base && m.terms == n.terms => Some(m),
            _ => None,
        });
        let Some(m) = same else {
            out.push(p);
            continue;
        };
        let values: Vec<Value> = count_reachable(n)
            .last()
            .unwrap()
            .iter()
            .filter(|&&t| count_accepts(n, t) && count_accepts(m, t))
            .map(|&t| Value::Float(t as f64 / 1000.0))
            .collect();
        if values.is_empty() {
            return vec![Pred::Or(Vec::new())];
        }
        m.op = Op::In;
        m.values = values;
    }
    out
}

/// The last parsed condition of a counter term: failing it alone keeps
/// the step from running, without negating guards the term shares with
/// the rest of the row (`a in (0,1,2,3)` around `a = 2`).
fn step_condition(term: &Pred) -> &Pred {
    match term {
        Pred::And(c) => c
            .iter()
            .rev()
            .find(|p| !matches!(p, Pred::Tautology))
            .map_or(term, step_condition),
        other => other,
    }
}

/// `var` through any chain of [`collect_aliases`] copies.
fn resolve_alias(aliases: &HashMap<String, String>, mut var: String) -> String {
    let mut hops = 0;
    while let Some(src) = aliases.get(&var) {
        hops += 1;
        if hops > aliases.len() {
            break;
        }
        var = src.clone();
    }
    var
}

/// SAS automatic variables: lexed as identifiers, never dataset columns.
const AUTOMATIC_VARIABLES: [&str; 7] = [
    "_n_", "_error_", "_iorc_", "_infile_", "_file_", "_cmd_", "_msg_",
];

/// Variables read on the right-hand side of an assignment (`x = y * 2;`,
/// `then x = y;`, `do i = 1 to n;`). Function names, array element
/// references and format names (`yymmdd8.`) are not variables. Combined
/// with [`collect_assigned`] this finds inputs that never appear in an
/// `if` head, such as `ht = raw_ht / 100;`.
fn collect_read(tokens: &[Token]) -> HashSet<String> {
    let mut out = HashSet::new();
    for i in 1..tokens.len() {
        if tokens[i].token_type != TokenType::ASSIGN
            || tokens[i - 1].token_type != TokenType::Identifier
            || !statement_context(tokens, i - 1)
        {
            continue;
        }
        let mut j = i + 1;
        while j < tokens.len() && tokens[j].token_type != TokenType::SEMI {
            let t = &tokens[j];
            if t.token_type == TokenType::Identifier
                && !is(tokens, j + 1, TokenType::LPAREN)
                && !is(tokens, j + 1, TokenType::LBRACK)
                && !is(tokens, j + 1, TokenType::DOT)
                && !AUTOMATIC_VARIABLES.iter().any(|a| t.text.eq_ignore_ascii_case(a))
                // `sum(of a1-a3)`: sas-lexer has no keyword type for `of`,
                // which only means "variable list" as a call's first word.
                && !(t.text.eq_ignore_ascii_case("of")
                    && (is(tokens, j - 1, TokenType::LPAREN) || is(tokens, j - 1, TokenType::COMMA)))
            {
                out.insert(t.text.clone());
            }
            j += 1;
        }
    }
    out
}

/// Plain copies `X = Y;` that are `X`'s only assignment and sit outside
/// any `if` / `do` block, mapped `X → Y`. Self-report recode sections
/// (`smoke = raw_smoke;`) route every input through one, so predicates on `X`
/// are really predicates on the input `Y`.
fn collect_aliases(tokens: &[Token]) -> HashMap<String, String> {
    let mut writes: HashMap<&str, usize> = HashMap::new();
    for i in 1..tokens.len() {
        let lhs = &tokens[i - 1];
        if tokens[i].token_type == TokenType::ASSIGN
            && lhs.token_type == TokenType::Identifier
            && statement_context(tokens, i - 1)
        {
            *writes.entry(lhs.text.as_str()).or_default() += 1;
        }
    }
    let mut out = HashMap::new();
    let mut depth = 0i32;
    for i in 0..tokens.len() {
        match tokens[i].token_type {
            TokenType::KwDo => depth += 1,
            TokenType::KwEnd => depth -= 1,
            _ => {}
        }
        let at_statement_start = i == 0 || is(tokens, i - 1, TokenType::SEMI);
        if depth == 0
            && at_statement_start
            && is(tokens, i, TokenType::Identifier)
            && is(tokens, i + 1, TokenType::ASSIGN)
            && is(tokens, i + 2, TokenType::Identifier)
            && is(tokens, i + 3, TokenType::SEMI)
            && tokens[i].text != tokens[i + 2].text
            && writes.get(tokens[i].text.as_str()) == Some(&1)
        {
            out.insert(tokens[i].text.clone(), tokens[i + 2].text.clone());
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
    aliases: &'a HashMap<String, String>,
    used: HashSet<String>,
    preds: Vec<Pred>,
    branches: Vec<Branch>,
    /// Constant assignments `X = literal;` / `X = .;`, in source order,
    /// with the condition under which each runs.
    defs: Vec<Def>,
    /// Variables with some assignment that isn't a constant.
    computed: HashSet<String>,
}

/// One assignment to a derived variable of a constant or a copy of
/// another variable, and the condition that reaches it (enclosing guards,
/// its `then`, negated earlier `else` arms).
#[derive(Clone, Debug)]
struct Def {
    var: String,
    value: DefValue,
    cond: Vec<Pred>,
}

#[derive(Clone, Debug)]
enum DefValue {
    Const(Value),
    Copy(String),
    /// `X = Y * k` (also `k * Y`, `Y / k`): a unit conversion.
    Scaled(String, f64),
    /// `X = X + k` (also `k + X`, `X - k`): a counter step.
    Add(f64),
    /// `X = X + Y` (also `Y + X`, `X + Y - k`): a counter step by a
    /// variable, plus a constant.
    AddVar(String, f64),
    /// The value a declared input arrives with.
    Input,
    /// `X = k - (a + b) + c …`: a constant plus each variable times its
    /// coefficient (±1), with the values each variable can take. SAS
    /// arithmetic is missing when any operand is.
    Linear(f64, Vec<LinearTerm>),
    /// `X = sum(a, b, …)`: like [`DefValue::Linear`], but SAS's `sum`
    /// skips missing arguments and is missing only when all of them are.
    Sum(Vec<LinearTerm>),
}

#[derive(Clone, Debug)]
struct LinearTerm {
    var: String,
    coeff: f64,
    /// Filled in by [`linear_values`] once the graph is built.
    values: Vec<f64>,
    /// A comparison in parentheses (`(a = 0)`), 1 when it holds and 0
    /// otherwise, in place of a variable.
    test: Option<Pred>,
    /// The arguments of a `max(…)` in place of a variable: their largest
    /// value, missing when all are.
    max_of: Vec<String>,
}

enum Operand {
    Var(String),
    /// The tokens of a parenthesised comparison, by index into the rhs.
    Test(RangeInclusive<usize>),
}

enum Step {
    By(f64),
    ByVar(String, f64),
}

/// `X + k`, `k + X`, `X - k`, `X + Y`, `Y + X` or `X + Y ± k` for
/// counter `X`, as its step, from the tokens between `=` and `;`.
fn counter_step(var: &str, rhs: &[Token]) -> Option<Step> {
    let num = |t: &Token| match t.token_type {
        TokenType::IntegerLiteral | TokenType::FloatLiteral | TokenType::FloatExponentLiteral => {
            parse_lit(t).as_f64()
        }
        _ => None,
    };
    let is_var = |t: &Token| t.token_type == TokenType::Identifier && t.text == var;
    let step = |t: &Token| match t.token_type {
        TokenType::Identifier if t.text != var => Some(Step::ByVar(t.text.clone(), 0.0)),
        _ => num(t).map(Step::By),
    };
    match rhs {
        [x, op, k] if is_var(x) && op.token_type == TokenType::PLUS => step(k),
        [x, op, k] if is_var(x) && op.token_type == TokenType::MINUS => {
            num(k).map(|k| Step::By(-k))
        }
        [k, op, x] if is_var(x) && op.token_type == TokenType::PLUS => step(k),
        [x, plus, y, sign, k]
            if is_var(x)
                && plus.token_type == TokenType::PLUS
                && y.token_type == TokenType::Identifier
                && y.text != var =>
        {
            let k = num(k)?;
            match sign.token_type {
                TokenType::PLUS => Some(Step::ByVar(y.text.clone(), k)),
                TokenType::MINUS => Some(Step::ByVar(y.text.clone(), -k)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// `k - (a + b + c)` and other sums and differences of numbers and
/// variables in parentheses, as the constant and each variable's
/// coefficient, from the tokens between `=` and `;`. `None` for anything
/// else, including a sum that mentions `var` itself (a counter step) or a
/// single operand (a constant or copy).
fn linear_sum(var: &str, rhs: &[Token]) -> Option<(f64, Vec<(Operand, f64)>)> {
    let mut base = 0.0;
    let mut terms: Vec<(Operand, f64)> = Vec::new();
    // Sign each open parenthesis applies, and the sign of the next operand.
    let mut signs = vec![1.0];
    let mut sign = 1.0;
    let mut expect_operand = true;
    let mut operands = 0;
    let mut skip_to = 0;
    for (i, t) in rhs.iter().enumerate() {
        if i < skip_to {
            continue;
        }
        let outer = *signs.last()?;
        match t.token_type {
            TokenType::LPAREN if expect_operand && comparison_in_parens(rhs, i).is_some() => {
                let close = comparison_in_parens(rhs, i)?;
                terms.push((Operand::Test(i + 1..=close - 1), outer * sign));
                skip_to = close + 1;
                (sign, expect_operand, operands) = (1.0, false, operands + 1);
            }
            TokenType::LPAREN if expect_operand => {
                signs.push(outer * sign);
                sign = 1.0;
            }
            TokenType::RPAREN if !expect_operand && signs.len() > 1 => {
                signs.pop();
            }
            TokenType::PLUS | TokenType::MINUS => {
                if t.token_type == TokenType::MINUS {
                    sign = -sign;
                }
                expect_operand = true;
            }
            TokenType::IntegerLiteral
            | TokenType::FloatLiteral
            | TokenType::FloatExponentLiteral
                if expect_operand =>
            {
                base += outer * sign * parse_lit(t).as_f64()?;
                (sign, expect_operand, operands) = (1.0, false, operands + 1);
            }
            TokenType::Identifier if expect_operand && t.text != var => {
                let coeff = outer * sign;
                match terms
                    .iter_mut()
                    .find(|(v, _)| matches!(v, Operand::Var(v) if *v == t.text))
                {
                    Some((_, c)) => *c += coeff,
                    None => terms.push((Operand::Var(t.text.clone()), coeff)),
                }
                (sign, expect_operand, operands) = (1.0, false, operands + 1);
            }
            _ => return None,
        }
    }
    (signs.len() == 1 && !expect_operand && operands > 1 && !terms.is_empty())
        .then_some((base, terms))
}

/// An argument of [`sum_call`].
enum SumArg {
    Var(String),
    /// `max(a, b)` or `max(of a b)`.
    Max(Vec<String>),
}

/// `sum(a, b, …)` over plain variables and `max(…)`s of them, as its
/// arguments.
fn sum_call(rhs: &[Token]) -> Option<Vec<SumArg>> {
    let args = call_args(rhs, "sum")?;
    args.iter()
        .map(|arg| match arg {
            [t] if t.token_type == TokenType::Identifier => Some(SumArg::Var(t.text.clone())),
            _ => {
                let inner = call_args(arg, "max")?;
                let mut vars = Vec::new();
                for (k, piece) in inner.iter().enumerate() {
                    for (j, t) in piece.iter().enumerate() {
                        // `of` lexes as an identifier and only means
                        // "variable list" as the call's first word.
                        let of = k == 0 && j == 0 && t.text.eq_ignore_ascii_case("of");
                        if t.token_type != TokenType::Identifier {
                            return None;
                        }
                        if !of {
                            vars.push(t.text.clone());
                        }
                    }
                }
                (!vars.is_empty()).then_some(SumArg::Max(vars))
            }
        })
        .collect()
}

/// The comma-separated arguments of `name( … )` when `tokens` is exactly
/// that call.
fn call_args<'t>(tokens: &'t [Token], name: &str) -> Option<Vec<&'t [Token]>> {
    let [call, open, inner @ .., close] = tokens else {
        return None;
    };
    if call.token_type != TokenType::Identifier
        || !call.text.eq_ignore_ascii_case(name)
        || open.token_type != TokenType::LPAREN
        || close.token_type != TokenType::RPAREN
    {
        return None;
    }
    let mut args = Vec::new();
    let (mut depth, mut start) = (0, 0);
    for (k, t) in inner.iter().enumerate() {
        match t.token_type {
            TokenType::LPAREN => depth += 1,
            TokenType::RPAREN => depth -= 1,
            TokenType::COMMA if depth == 0 => {
                args.push(&inner[start..k]);
                start = k + 1;
            }
            _ => {}
        }
    }
    args.push(&inner[start..]);
    args.iter().all(|a| !a.is_empty()).then_some(args)
}

/// The index of the `)` closing the `(` at `open` when the tokens between
/// hold a comparison operator outside any nested parentheses: `(a = 0)`
/// is SAS's 1-or-0 test, where `(a + b)` is a grouping.
fn comparison_in_parens(tokens: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0;
    let mut compares = false;
    for (j, t) in tokens.iter().enumerate().skip(open) {
        match t.token_type {
            TokenType::LPAREN => depth += 1,
            TokenType::RPAREN => {
                depth -= 1;
                if depth == 0 {
                    return compares.then_some(j);
                }
            }
            TokenType::KwIN => compares |= depth == 1,
            ty if cmp_op(ty).is_some() => compares |= depth == 1,
            _ => {}
        }
    }
    None
}

/// `Y * k`, `k * Y` or `Y / k` (optionally wrapped in one pair of parens)
/// as `(Y, k)`, from the tokens between `=` and `;`.
fn scaled_copy(rhs: &[Token]) -> Option<(String, f64)> {
    let inner = match rhs {
        [l, mid @ .., r]
            if l.token_type == TokenType::LPAREN && r.token_type == TokenType::RPAREN =>
        {
            mid
        }
        all => all,
    };
    let num = |t: &Token| match t.token_type {
        TokenType::IntegerLiteral | TokenType::FloatLiteral | TokenType::FloatExponentLiteral => {
            parse_lit(t).as_f64()
        }
        _ => None,
    };
    match inner {
        [y, op, k] if y.token_type == TokenType::Identifier && op.token_type == TokenType::STAR => {
            Some((y.text.clone(), num(k)?))
        }
        [k, op, y] if y.token_type == TokenType::Identifier && op.token_type == TokenType::STAR => {
            Some((y.text.clone(), num(k)?))
        }
        [y, op, k]
            if y.token_type == TokenType::Identifier && op.token_type == TokenType::FSLASH =>
        {
            let k = num(k)?;
            (k != 0.0).then(|| (y.text.clone(), 1.0 / k))
        }
        _ => None,
    }
    .filter(|(_, k)| *k != 0.0)
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
        // Reach for the single statement after a `then`.
        let mut then_arm: Option<Vec<Pred>> = None;
        let start = i;
        while i < tokens.len() {
            let statement_start = i == start
                || matches!(
                    ty(tokens, i - 1),
                    Some(TokenType::SEMI | TokenType::KwThen | TokenType::KwElse)
                );
            if statement_start
                && is(tokens, i, TokenType::Identifier)
                && is(tokens, i + 1, TokenType::ASSIGN)
            {
                let var = tokens[i].text.clone();
                let single = is(tokens, i + 3, TokenType::SEMI);
                let noop =
                    single && is(tokens, i + 2, TokenType::Identifier) && tokens[i + 2].text == var;
                let value = if noop {
                    None
                } else if single && is(tokens, i + 2, TokenType::DOT) {
                    Some(DefValue::Const(Value::Missing))
                } else if single && ty(tokens, i + 2).is_some_and(is_literal) {
                    Some(DefValue::Const(parse_lit(&tokens[i + 2])))
                } else if single
                    && is(tokens, i + 2, TokenType::Identifier)
                    && tokens[i + 2].text != var
                    && !self.arrays.contains_key(&tokens[i + 2].text)
                {
                    Some(DefValue::Copy(resolve_alias(
                        self.aliases,
                        tokens[i + 2].text.clone(),
                    )))
                } else {
                    let end = statement_end(tokens, i + 2);
                    let rhs = &tokens[i + 2..end];
                    let step = counter_step(&var, rhs).map(|step| match step {
                        Step::By(k) => DefValue::Add(k),
                        Step::ByVar(y, k) => DefValue::AddVar(resolve_alias(self.aliases, y), k),
                    });
                    step.or_else(|| {
                        scaled_copy(rhs)
                            .map(|(src, k)| DefValue::Scaled(resolve_alias(self.aliases, src), k))
                    })
                    .or_else(|| {
                        sum_call(rhs).map(|args| {
                            DefValue::Sum(
                                args.into_iter()
                                    .map(|arg| match arg {
                                        SumArg::Var(v) => LinearTerm {
                                            var: resolve_alias(self.aliases, v),
                                            coeff: 1.0,
                                            values: Vec::new(),
                                            test: None,
                                            max_of: Vec::new(),
                                        },
                                        SumArg::Max(vars) => LinearTerm {
                                            var: String::new(),
                                            coeff: 1.0,
                                            values: Vec::new(),
                                            test: None,
                                            max_of: vars
                                                .into_iter()
                                                .map(|v| resolve_alias(self.aliases, v))
                                                .collect(),
                                        },
                                    })
                                    .collect(),
                            )
                        })
                    })
                    .or_else(|| {
                        linear_sum(&var, rhs).map(|(base, terms)| {
                            let terms = terms
                                .into_iter()
                                .map(|(operand, coeff)| match operand {
                                    Operand::Var(v) => LinearTerm {
                                        var: resolve_alias(self.aliases, v),
                                        coeff,
                                        values: Vec::new(),
                                        test: None,
                                        max_of: Vec::new(),
                                    },
                                    Operand::Test(range) => LinearTerm {
                                        var: String::new(),
                                        coeff,
                                        values: Vec::new(),
                                        max_of: Vec::new(),
                                        test: Some(
                                            PredicateParser::new(
                                                &rhs[range],
                                                self.arrays,
                                                self.aliases,
                                                &mut self.used,
                                            )
                                            .parse(),
                                        ),
                                    },
                                })
                                .collect();
                            DefValue::Linear(base, terms)
                        })
                    })
                };
                if let Some(value) = value {
                    let cond = then_arm
                        .clone()
                        .or_else(|| arm.clone())
                        .unwrap_or_else(|| reach.clone());
                    self.defs.push(Def { var, value, cond });
                } else if !noop {
                    self.computed.insert(var);
                }
            } else if statement_start
                && is(tokens, i, TokenType::Identifier)
                && is(tokens, i + 1, TokenType::PLUS)
            {
                // A sum statement, `x + k;`. SAS starts `x` at 0, keeps it
                // from row to row and adds a missing `k` as 0, but as a
                // step after the counter is reset it is `x = x + k;`.
                let var = tokens[i].text.clone();
                let end = statement_end(tokens, i);
                match counter_step(&var, &tokens[i..end]) {
                    Some(step) => {
                        let value = match step {
                            Step::By(k) => DefValue::Add(k),
                            Step::ByVar(y, k) => {
                                DefValue::AddVar(resolve_alias(self.aliases, y), k)
                            }
                        };
                        let cond = then_arm
                            .clone()
                            .or_else(|| arm.clone())
                            .unwrap_or_else(|| reach.clone());
                        self.defs.push(Def { var, value, cond });
                    }
                    None => {
                        self.computed.insert(var);
                    }
                }
            }
            match tokens[i].token_type {
                TokenType::KwIf => {
                    let in_else = arm.is_some();
                    let here = arm.take().unwrap_or_else(|| reach.clone());
                    let j = if_head_end(tokens, i + 1);
                    let pred = PredicateParser::new(
                        &tokens[i + 1..j],
                        self.arrays,
                        self.aliases,
                        &mut self.used,
                    )
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
                        let mut taken = here;
                        taken.push(pred);
                        then_arm = Some(taken);
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
                    then_arm = None;
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

/// The operator that keeps a comparison true with its operands swapped
/// (`3 < x` ⇔ `x > 3`).
fn flip(op: Op) -> Op {
    match op {
        Op::Lt => Op::Gt,
        Op::Le => Op::Ge,
        Op::Gt => Op::Lt,
        Op::Ge => Op::Le,
        other => other,
    }
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
    aliases: &'a HashMap<String, String>,
    used: &'a mut HashSet<String>,
}

impl<'a> PredicateParser<'a> {
    fn new(
        t: &'a [Token],
        arrays: &'a HashMap<String, ArrayDecl>,
        aliases: &'a HashMap<String, String>,
        used: &'a mut HashSet<String>,
    ) -> Self {
        PredicateParser {
            t,
            i: 0,
            arrays,
            aliases,
            used,
        }
    }

    /// The variable `var` copies, through any chain of [`collect_aliases`].
    fn resolve(&self, var: String) -> String {
        resolve_alias(self.aliases, var)
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
                if let Some(inner) = inner {
                    return Some(self.compared_test(inner));
                }
            }
            self.i = start;
            self.skip_paren();
            return Some(Pred::Tautology);
        }
        Some(self.parse_atom())
    }

    /// `(test) = 1`, `(test) ne 0` and the like: SAS evaluates a
    /// parenthesised comparison to 1 or 0, so comparing that with 1 or 0
    /// is the test or its negation. Any other comparison of it is unknown.
    fn compared_test(&mut self, inner: Pred) -> Pred {
        let Some(op) = ty(self.t, self.i).and_then(cmp_op) else {
            return inner;
        };
        let save = self.i;
        self.i += 1;
        let holds_when = match (op, self.consume_literal().and_then(|v| v.as_f64())) {
            (Op::Eq, Some(1.0)) | (Op::Ne, Some(0.0)) => Some(true),
            (Op::Eq, Some(0.0)) | (Op::Ne, Some(1.0)) => Some(false),
            (_, Some(_)) => None,
            (_, None) => {
                self.i = save;
                return inner;
            }
        };
        match holds_when {
            Some(true) => inner,
            Some(false) => Pred::Not(Box::new(inner)),
            None => Pred::Tautology,
        }
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

        if self.peek(TokenType::KwMissing)
            && next_lparen
            && self.peek_at(self.i + 2, TokenType::Identifier)
            && self.peek_at(self.i + 3, TokenType::RPAREN)
        {
            let var = self.t[self.i + 2].text.clone();
            self.i += 4;
            return self.atom(var, Op::Eq, vec![Value::Missing]);
        }
        if ident && next_lparen && self.t[self.i].text.eq_ignore_ascii_case("sum") {
            if let Some(p) = self.parse_sum() {
                return p;
            }
        }
        if ident && next_lparen && self.t[self.i].text.eq_ignore_ascii_case("max") {
            let save = self.i;
            if let Some(p) = self.parse_max() {
                return p;
            }
            self.i = save;
        }
        if ty(self.t, self.i).is_some_and(|t| is_literal(t) || t == TokenType::DOT)
            && ty(self.t, self.i + 1).and_then(cmp_op).is_some()
            && self.peek_at(self.i + 2, TokenType::Identifier)
            && !self.peek_at(self.i + 3, TokenType::LPAREN)
            && !self.peek_at(self.i + 3, TokenType::LBRACK)
        {
            return self.parse_literal_first_atom();
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
        let save = self.i;
        let var = self.t[self.i].text.clone();
        // `x not in (…)`, `x not eq k`: the comparison, negated.
        let negated = (self.peek_at(self.i + 1, TokenType::KwNOT)
            || self.peek_at(self.i + 1, TokenType::NOT))
            && (self.peek_at(self.i + 2, TokenType::KwIN)
                || ty(self.t, self.i + 2).and_then(cmp_op).is_some());
        self.i += if negated { 2 } else { 1 };
        let p = self.parse_comparison(var);
        if p.is_none() {
            self.i = save;
        }
        p.map(|p| if negated { Pred::Not(Box::new(p)) } else { p })
    }

    /// The `in (…)` list or comparison after `var`, at the cursor, or
    /// `- y op k` comparing the difference of `var` and `y` with `k`.
    fn parse_comparison(&mut self, var: String) -> Option<Pred> {
        if self.peek(TokenType::MINUS)
            && self.peek_at(self.i + 1, TokenType::Identifier)
            && !self.arrays.contains_key(&var)
            && !self.arrays.contains_key(&self.t[self.i + 1].text)
        {
            let save = self.i;
            let right = self.t[self.i + 1].text.clone();
            if let Some(op) = ty(self.t, self.i + 2).and_then(cmp_op) {
                self.i += 3;
                if let Some(k) = self.consume_literal().and_then(|v| v.as_f64()) {
                    let (left, right) = (self.resolve(var), self.resolve(right));
                    self.used.insert(left.clone());
                    self.used.insert(right.clone());
                    return Some(Pred::Cmp(CmpAtom {
                        left,
                        op,
                        right,
                        diff: Some(k),
                    }));
                }
            }
            self.i = save;
            return None;
        }
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
            if self.peek(TokenType::Identifier)
                && !self.peek_at(self.i + 1, TokenType::LPAREN)
                && !self.peek_at(self.i + 1, TokenType::LBRACK)
                && !self.arrays.contains_key(&var)
                && !self.arrays.contains_key(&self.t[self.i].text)
            {
                let right = self.t[self.i].text.clone();
                self.i += 1;
                let (left, right) = (self.resolve(var), self.resolve(right));
                self.used.insert(left.clone());
                self.used.insert(right.clone());
                return Some(Pred::Cmp(CmpAtom {
                    left,
                    op,
                    right,
                    diff: None,
                }));
            }
        }
        None
    }

    /// `LIT op IDENT`, optionally chained as `LIT op IDENT op LIT`
    /// (`5 le bmi le 70`, `. <= days <= 7`): the comparison is turned round
    /// to read `IDENT op' LIT`, and a chain becomes the conjunction of both
    /// ends.
    fn parse_literal_first_atom(&mut self) -> Pred {
        let lo = self.consume_literal().expect("checked by caller");
        let op = flip(cmp_op(self.t[self.i].token_type).expect("checked by caller"));
        let var = self.t[self.i + 1].text.clone();
        self.i += 2;
        let first = self.atom(var.clone(), op, vec![lo]);
        if let Some(op2) = ty(self.t, self.i).and_then(cmp_op) {
            let save = self.i;
            self.i += 1;
            if let Some(hi) = self.consume_literal() {
                let second = self.atom(var, op2, vec![hi]);
                return Pred::And(vec![first, second]);
            }
            self.i = save;
        }
        first
    }

    fn atom(&mut self, var: String, op: Op, values: Vec<Value>) -> Pred {
        let var = self.resolve(var);
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
        let args = self.parse_call_args()?;
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

    /// `max( ident, … ) cmp_op LITERAL` or `max( … ) IN ( … )`. A SAS
    /// missing value sorts below every number, and `max` of all-missing
    /// arguments is missing, so the maximum is the plain ordering maximum:
    /// it is at least `k` when any argument is, at most `k` when every
    /// argument is, and equals `k` when both an argument equals `k` and
    /// every argument is at most `k`.
    fn parse_max(&mut self) -> Option<Pred> {
        let args = self.parse_call_args()?;
        let (op, values) =
            if self.peek(TokenType::KwIN) && self.peek_at(self.i + 1, TokenType::LPAREN) {
                self.i += 2;
                (Op::In, self.consume_literal_list_until_rparen())
            } else {
                let op = ty(self.t, self.i).and_then(cmp_op)?;
                self.i += 1;
                (op, vec![self.consume_literal()?])
            };
        if args.iter().any(|a| self.arrays.contains_key(a)) || values.is_empty() {
            return None;
        }
        let mut each = |op: Op, v: &Value| -> Vec<Pred> {
            args.iter()
                .map(|a| self.atom(a.clone(), op, vec![v.clone()]))
                .collect()
        };
        let mut equals =
            |v: &Value| Pred::And(vec![Pred::Or(each(Op::Eq, v)), Pred::And(each(Op::Le, v))]);
        Some(match op {
            Op::Gt | Op::Ge => Pred::Or(each(op, &values[0])),
            Op::Lt | Op::Le => Pred::And(each(op, &values[0])),
            Op::Eq | Op::In => Pred::Or(values.iter().map(&mut equals).collect()),
            Op::Ne => Pred::Not(Box::new(Pred::Or(values.iter().map(&mut equals).collect()))),
        })
    }

    /// The identifiers passed to the call at the cursor, `name( a, b )`,
    /// leaving the cursor after its `)`.
    fn parse_call_args(&mut self) -> Option<Vec<String>> {
        self.i += 2; // name (
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
                // `max(of a b)`: `of` lexes as an identifier, and only
                // means "variable list" as the call's first word.
                TokenType::Identifier
                    if depth == 1
                        && !(args.is_empty()
                            && t.text.eq_ignore_ascii_case("of")
                            && self.peek_at(self.i - 1, TokenType::LPAREN)) =>
                {
                    args.push(self.resolve(t.text.clone()))
                }
                _ => {}
            }
            self.i += 1;
        }
        if !self.peek(TokenType::RPAREN) {
            return None;
        }
        self.i += 1;
        Some(args)
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
                // A lone `.` is SAS's numeric missing value: `in (., 0, 1)`.
                TokenType::DOT if depth == 1 => values.push(Value::Missing),
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
        // A lone `.` is SAS's numeric missing value: `x = .`, `x ne .`.
        if self.peek(TokenType::DOT) {
            self.i += 1;
            return Some(Value::Missing);
        }
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
        Pred::Atom(_) | Pred::Sum(_) | Pred::Cmp(_) => f(node),
        Pred::Count(c) => c.terms.iter().for_each(|(p, _)| walk_atom(p, f)),
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
            set.extend(metadata_values(&spec.kind));
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
fn metadata_values(kind: &SpecKind) -> Vec<Value> {
    match kind {
        SpecKind::Range(lo, hi) => {
            if hi < lo {
                return Vec::new();
            }
            let size = hi - lo + 1;
            if size <= 9 {
                return (*lo..=*hi).map(Value::Int).collect();
            }
            let mut out = vec![*lo];
            out.extend((1..=7).map(|k| lo + size * k / 8));
            out.push(*hi);
            out.dedup();
            out.into_iter().map(Value::Int).collect()
        }
        SpecKind::Set(values) => values.iter().copied().map(Value::Int).collect(),
        SpecKind::Decimal(lo, hi) => decimal_slices(*lo, *hi),
        // Spread over a century, in order, so date comparisons go both ways.
        SpecKind::Date => [
            "19250101", "19400615", "19601231", "19850301", "20200101", "20251231",
        ]
        .into_iter()
        .map(|d| Value::Str(d.to_string()))
        .collect(),
    }
}

/// The edges of a documented domain [`minimal`] makes sure some row hits:
/// its min and max, and one step outside each (1 for integer specs; the
/// finest decimal place of the bounds, at least 0.1, for decimal ranges).
fn boundary_values(kind: &SpecKind) -> Vec<Value> {
    let ints = |lo: i64, hi: i64| {
        let mut out: Vec<Value> = [lo - 1, lo, hi, hi + 1]
            .into_iter()
            .map(Value::Int)
            .collect();
        out.dedup();
        out
    };
    match kind {
        SpecKind::Range(lo, hi) if lo <= hi => ints(*lo, *hi),
        SpecKind::Set(values) => match (values.iter().min(), values.iter().max()) {
            (Some(lo), Some(hi)) => ints(*lo, *hi),
            _ => Vec::new(),
        },
        SpecKind::Decimal(lo, hi) if lo <= hi => {
            let places = |x: f64| x.to_string().split('.').nth(1).map_or(0, str::len);
            let step = 10f64.powi(-(places(*lo).max(places(*hi)).max(1) as i32));
            let round = |x: f64| Value::Float((x * 1e9).round() / 1e9);
            let mut out = vec![round(lo - step), round(*lo), round(*hi), round(hi + step)];
            out.dedup();
            out
        }
        _ => Vec::new(),
    }
}

/// The bounds of [`DECIMAL_SLICES`] equal slices of `lo..=hi`.
fn decimal_slices(lo: f64, hi: f64) -> Vec<Value> {
    if hi < lo {
        return Vec::new();
    }
    let mut out: Vec<Value> = (0..=DECIMAL_SLICES)
        .map(|k| {
            let x = if k == DECIMAL_SLICES {
                hi
            } else {
                lo + (hi - lo) * k as f64 / DECIMAL_SLICES as f64
            };
            // Round off float noise (0.1 * 3 → 0.30000000000000004).
            Value::Float((x * 1e9).round() / 1e9)
        })
        .collect();
    out.dedup();
    out
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
/// Pin cells of `row` so `node` holds. Returns the cells it moved to the
/// middle of a two-sided range ([`centre_ranges`]).
fn satisfy(node: &Pred, row: &mut Row, g: &Graph, rng: &mut Rng) -> HashSet<String> {
    let mut centred = HashSet::new();
    match node {
        Pred::Atom(a) => satisfy_atom(a, row, g, rng),
        Pred::Sum(s) => satisfy_sum(s, row, g, rng),
        Pred::Cmp(c) => satisfy_cmp(c, row, g, rng),
        Pred::Count(c) => {
            // Cells already pinned decide steps that test them.
            let pinned: Vec<Atom> = row
                .iter()
                .map(|(var, v)| Atom {
                    var: var.clone(),
                    op: Op::Eq,
                    values: vec![v.clone()],
                    is_array: false,
                })
                .collect();
            let need = pick_within(&pick_count(c, &pinned, rng), &pinned, rng);
            centred.extend(satisfy(&need, row, g, rng));
        }
        Pred::And(c) => {
            let before: HashSet<String> = row.keys().cloned().collect();
            for p in c {
                centred.extend(satisfy(p, row, g, rng));
            }
            centred.extend(centre_ranges(node, row, &before, g));
        }
        Pred::Or(c) => {
            if let Some(p) = rng.choose(c) {
                centred.extend(satisfy(p, row, g, rng));
            }
        }
        Pred::Not(_) | Pred::Tautology => {}
    }
    centred
}

/// For each variable this conjunction just pinned that it also bounds on
/// both sides (`lo < x < hi`), move the pin to the known value nearest the
/// middle of the range, or the midpoint itself. Satisfying `satisfy`'s
/// first pick sits at an edge; a typical value keeps rows sensible for
/// arithmetic downstream that the graph doesn't model (`wt / (ht * ht)`).
fn centre_ranges(
    node: &Pred,
    row: &mut Row,
    before: &HashSet<String>,
    g: &Graph,
) -> HashSet<String> {
    let mut centred = HashSet::new();
    fn conjuncts<'p>(node: &'p Pred, out: &mut Vec<&'p Atom>) {
        match node {
            Pred::Atom(a) if !a.is_array => out.push(a),
            Pred::And(c) => c.iter().for_each(|p| conjuncts(p, out)),
            _ => {}
        }
    }
    let mut atoms = Vec::new();
    conjuncts(node, &mut atoms);
    let mut bounds: HashMap<&str, (Option<f64>, Option<f64>)> = HashMap::new();
    for a in &atoms {
        let Some(x) = a.values.first().and_then(Value::as_f64) else {
            continue;
        };
        let b = bounds.entry(a.var.as_str()).or_default();
        match a.op {
            Op::Gt | Op::Ge => b.0 = Some(b.0.map_or(x, |lo| lo.max(x))),
            Op::Lt | Op::Le => b.1 = Some(b.1.map_or(x, |hi| hi.min(x))),
            _ => {}
        }
    }
    for (var, (lo, hi)) in bounds {
        let (Some(lo), Some(hi)) = (lo, hi) else {
            continue;
        };
        if lo >= hi || before.contains(var) || !row.contains_key(var) {
            continue;
        }
        let mid = (lo + hi) / 2.0;
        let fits = |v: &Value| {
            atoms
                .iter()
                .filter(|a| a.var == var)
                .all(|a| holds(a.op, v, &a.values) == Some(true))
        };
        let empty = Vec::new();
        let known = g.value_sets.get(var).unwrap_or(&empty);
        // Whole numbers when everything known about the variable is whole.
        let centre = if known
            .iter()
            .all(|v| matches!(v, Value::Int(_) | Value::Missing))
        {
            Value::Int(mid.round() as i64)
        } else {
            Value::Float((mid * 1e6).round() / 1e6)
        };
        let best = if fits(&centre) {
            centre
        } else {
            match known.iter().filter(|v| fits(v)).min_by(|a, b| {
                let d = |v: &Value| (v.as_f64().unwrap_or(f64::MAX) - mid).abs();
                d(a).total_cmp(&d(b))
            }) {
                Some(v) => v.clone(),
                None => continue,
            }
        };
        if fits(&best) {
            row.insert(var.to_string(), best);
            centred.insert(var.to_string());
        }
    }
    centred
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

/// Pin whichever side of `left op right` is still free to a value that
/// makes the comparison hold, trying the free side's value set and the
/// neighbours of the pinned side.
/// Whether `c` holds with `l` on its left and `r` on its right.
fn cmp_holds(c: &CmpAtom, l: &Value, r: &Value) -> Option<bool> {
    let Some(k) = c.diff else {
        return holds(c.op, l, std::slice::from_ref(r));
    };
    let diff = match (l, r) {
        (Value::Missing, _) | (_, Value::Missing) => Value::Missing,
        _ => Value::number(l.as_f64()? - r.as_f64()?),
    };
    holds(c.op, &diff, &[Value::number(k)])
}

fn satisfy_cmp(c: &CmpAtom, row: &mut Row, g: &Graph, rng: &mut Rng) {
    let empty = Vec::new();
    let pick = |var: &str, row: &Row, rng: &mut Rng| -> Value {
        row.get(var).cloned().unwrap_or_else(|| {
            let vs = g.value_sets.get(var).unwrap_or(&empty);
            let non_missing: Vec<&Value> = vs.iter().filter(|v| **v != Value::Missing).collect();
            rng.choose(&non_missing)
                .map_or(Value::Missing, |v| (*v).clone())
        })
    };
    let (free, fixed, fixed_is_left) = match (row.contains_key(&c.left), row.contains_key(&c.right))
    {
        (true, true) => return,
        (true, false) => (c.right.as_str(), c.left.as_str(), true),
        (false, _) => {
            let v = pick(&c.right, row, rng);
            row.insert(c.right.clone(), v);
            (c.left.as_str(), c.right.as_str(), false)
        }
    };
    let anchor = row[fixed].clone();
    let holds_with = |v: &Value| {
        let (l, r) = if fixed_is_left {
            (&anchor, v)
        } else {
            (v, &anchor)
        };
        cmp_holds(c, l, r) == Some(true)
    };
    let vs = g.value_sets.get(free).unwrap_or(&empty);
    let mut pool: Vec<Value> = vs.clone();
    pool.extend([
        anchor.clone(),
        bump(anchor.clone(), -1),
        bump(anchor.clone(), 1),
    ]);
    if let (Some(k), Some(a)) = (c.diff, anchor.as_f64()) {
        for d in [-1.0, 0.0, 1.0] {
            pool.extend([Value::number(a + k + d), Value::number(a - k + d)]);
        }
    }
    let fits: Vec<&Value> = pool.iter().filter(|v| holds_with(v)).collect();
    let v = rng
        .choose(&fits)
        .map_or_else(|| pick(free, row, rng), |v| (*v).clone());
    row.insert(free.to_string(), v);
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
        Pred::Cmp(c) => Pred::Cmp(CmpAtom {
            op: match c.op {
                Op::In | Op::Eq => Op::Ne,
                Op::Ne => Op::Eq,
                Op::Lt => Op::Ge,
                Op::Le => Op::Gt,
                Op::Gt => Op::Le,
                Op::Ge => Op::Lt,
            },
            ..c.clone()
        }),
        Pred::Count(c) => Pred::Count(CountAtom {
            op: match c.op {
                Op::In | Op::Eq => Op::Ne,
                Op::Ne => Op::In,
                Op::Lt => Op::Ge,
                Op::Le => Op::Gt,
                Op::Gt => Op::Le,
                Op::Ge => Op::Lt,
            },
            ..c.clone()
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
        // Not(true) is false.
        Pred::And(c) if c.is_empty() => Pred::Or(Vec::new()),
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
        Pred::Cmp(c) => cmp_holds(c, row.get(&c.left)?, row.get(&c.right)?),
        Pred::Count(c) => {
            let mut total = c.base;
            for (p, k) in &c.terms {
                if eval(p, row, g, slot)? {
                    total += k;
                }
            }
            holds(c.op, &Value::Float(total), &c.values)
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
    fn literal_first_comparisons_and_chained_ranges() {
        let g = graph(
            "if 0 < age then a = 1;\n\
             if 5 le bmi le 70 then b = 1;\n",
        );
        assert_eq!(g.inputs, vec!["age", "bmi"]);
        assert!(matches!(&g.predicates[0], Pred::Atom(a) if a.var == "age" && a.op == Op::Gt));
        assert!(matches!(&g.predicates[1], Pred::And(c)
            if matches!(&c[0], Pred::Atom(a) if a.op == Op::Ge && a.values == vec![Value::Int(5)])
            && matches!(&c[1], Pred::Atom(a) if a.op == Op::Le && a.values == vec![Value::Int(70)])));
    }

    #[test]
    fn minimal_satisfies_literal_first_and_chained_guards() {
        // Columns are age, bmi.
        let (rows, cov) = minimal_rows("if 0 < age and 5 le bmi le 70 then do; ok = 1; end;\n");
        assert!(cov.uncovered.is_empty());
        let num = |s: &str| s.parse::<f64>().unwrap();
        assert!(rows
            .iter()
            .any(|r| num(&r[0]) > 0.0 && (5.0..=70.0).contains(&num(&r[1]))));
    }

    #[test]
    fn spaced_comparisons_read_as_one_operator() {
        let g = graph("if a > = 0 and b < = 3 then ok = 1;\n");
        assert!(matches!(&g.predicates[0], Pred::And(c)
            if matches!(&c[0], Pred::Atom(a) if a.var == "a" && a.op == Op::Ge)
            && matches!(&c[1], Pred::Atom(a) if a.var == "b" && a.op == Op::Le)));
    }

    #[test]
    fn statements_after_an_unmatched_end_are_walked() {
        let g = graph(
            "* if a in (0,1) then do;\n\
             if a = 1 then x = 1;\n\
             end;\n\
             if b = 1 then y = 1;\n",
        );
        assert_eq!(g.inputs, vec!["a", "b"]);
        assert_eq!(g.branches.len(), 2);
    }

    #[test]
    fn differences_of_two_variables_are_compared() {
        // Columns are a, b.
        let (rows, cov) = minimal_rows(
            "if a in (0:6) and b in (0:6) then do;\n\
             if a - b ge 1 then up = 1;\n\
             else if a - b le 0 then up = 0;\n\
             end;\n",
        );
        assert!(cov.unproduced.is_empty(), "{:?}", cov.unproduced);
        let n = |s: &str| s.parse::<i64>().ok();
        let diffs: Vec<i64> = rows
            .iter()
            .filter_map(|r| Some(n(&r[0])? - n(&r[1])?))
            .collect();
        assert!(diffs.iter().any(|&d| d >= 1) && diffs.iter().any(|&d| d <= 0));
    }

    #[test]
    fn differences_of_derived_variables_are_compared() {
        // Columns are a, b. `x - y ge 2` holds only at a = 1, b = 0.
        let (rows, cov) = minimal_rows(
            "if a in (0,1) and b in (0,1) then do;\n\
             if a = 1 then x = 3; else x = 1;\n\
             if b = 1 then y = 2; else y = 1;\n\
             if x - y ge 2 then far = 1; else far = 0;\n\
             end;\n",
        );
        assert!(cov.unproduced.is_empty(), "{:?}", cov.unproduced);
        assert!(rows.iter().any(|r| r == &["1", "0"]), "{rows:?}");
    }

    #[test]
    fn chained_ranges_may_start_at_missing() {
        // `. <= days` holds for every value, missing included.
        let g = graph("if (. <= days <= 7) then ok = 1;\n");
        assert!(matches!(&g.predicates[0], Pred::And(c)
            if matches!(&c[0], Pred::Atom(a) if a.var == "days" && a.op == Op::Ge
                && a.values == vec![Value::Missing])));
        let (rows, cov) = minimal_rows("if (. <= days <= 7) then do; ok = 1; end;\n");
        assert!(cov.uncovered.is_empty());
        assert!(rows
            .iter()
            .any(|r| r[0].is_empty() || r[0].parse::<f64>().is_ok_and(|d| d <= 7.0)));
    }

    #[test]
    fn missing_and_variable_comparisons_are_modelled() {
        let g = graph("if not(missing(dob)) and dob < ref then age = 1;\n");
        assert_eq!(g.inputs, vec!["dob", "ref"]);
        assert!(matches!(&g.predicates[0], Pred::And(c)
            if matches!(&c[0], Pred::Not(p) if matches!(&**p, Pred::Atom(a)
                if a.var == "dob" && a.values == vec![Value::Missing]))
            && matches!(&c[1], Pred::Cmp(c) if c.left == "dob" && c.op == Op::Lt && c.right == "ref")));
    }

    #[test]
    fn minimal_drives_variable_comparisons_both_ways() {
        // Columns are a, b.
        let (rows, cov) = minimal_rows("if a < b then x = 1;\nelse x = 2;\n");
        assert!(cov.uncovered.is_empty());
        let num = |s: &str| s.parse::<f64>().unwrap();
        assert!(rows.iter().any(|r| num(&r[0]) < num(&r[1])));
        assert!(rows.iter().any(|r| num(&r[0]) >= num(&r[1])));
    }

    #[test]
    fn conditions_on_derived_constants_reach_the_inputs_deciding_them() {
        // Columns are c.
        let (rows, cov) = minimal_rows(
            "if c = 1 then flag = 1; else flag = 0;\n\
             if flag in (1) then x = 1;\n",
        );
        assert!(cov.uncovered.is_empty(), "{:?}", cov.uncovered);
        assert!(rows.iter().any(|r| r[0] == "1"));
        assert!(rows.iter().any(|r| r[0] != "1"));
    }

    #[test]
    fn the_last_constant_assignment_wins() {
        let g = graph("flag = 0;\nif c = 1 then flag = 1;\nif flag = 0 then x = 1;\n");
        // flag = 0 survives only where the later `flag = 1` doesn't fire.
        let leaf = &g.branches.last().unwrap().leaf;
        let row = |c: i64| HashMap::from([("c".to_string(), Value::Int(c))]);
        assert_eq!(eval(leaf, &row(1), &g, None), Some(false));
        assert_eq!(eval(leaf, &row(2), &g, None), Some(true));
    }

    #[test]
    fn guards_over_many_derived_indicators_become_reachable() {
        // Columns are a, b.
        let (rows, cov) = minimal_rows(
            "if a = 0 then ia = 0; else if a in (1, 2) then ia = 1;\n\
             if b = 0 then ib = 0; else if b = 1 then ib = 1;\n\
             if ia in (0, 1) and ib in (0, 1) then do; score = 1; end;\n",
        );
        assert!(cov.uncovered.is_empty(), "{:?}", cov.uncovered);
        let ok = |r: &Vec<String>| {
            ["0", "1", "2"].contains(&r[0].as_str()) && ["0", "1"].contains(&r[1].as_str())
        };
        assert!(rows.iter().any(ok));
        assert!(rows.iter().any(|r| !ok(r)));
    }

    #[test]
    fn computed_variables_are_not_treated_as_constants() {
        let g = graph("n = 0;\nif a = 1 then n = a * b;\nif n = 0 then x = 1;\n");
        // `n = 0` is not rewritten into a condition on `a`, so it stays a
        // branch over a derived variable and is not a coverage target.
        assert_eq!(g.branches.len(), 1);
    }

    #[test]
    fn counters_become_counts_of_their_increments() {
        let g = graph(
            "n = 0;\nif a = 1 then n = n + 1;\nif b in (1, 2) then n = n + 2;\n\
             if n = 2 then x = 1;\n",
        );
        assert_eq!(g.branches.len(), 3);
        let row = |a: i64, b: i64| -> Row {
            [("a", a), ("b", b)]
                .into_iter()
                .map(|(k, v)| (k.to_string(), Value::Int(v)))
                .collect()
        };
        let leaf = &g.branches[2].leaf;
        assert_eq!(eval(leaf, &row(0, 1), &g, None), Some(true));
        assert_eq!(eval(leaf, &row(1, 1), &g, None), Some(false));
        assert_eq!(eval(leaf, &row(1, 0), &g, None), Some(false));
    }

    #[test]
    fn minimal_produces_each_counter_total_and_assigned_value() {
        // Columns are a, b, c. Only rows passing the guard count, and
        // `s = 0` needs every increment off at once.
        let (rows, cov) = minimal_rows(
            "if a in (0, 1) and b in (0, 1) and c in (0, 1) then do;\n\
               n = 0;\n\
               if a = 1 then n = n + 1;\n\
               if b = 1 then n = n + 1;\n\
               if c = 1 then n = n + 1;\n\
               if n = 0 then s = 0; else if n = 1 then s = 1; else if n ge 2 then s = 2;\n\
             end;\n",
        );
        // Past n = 0 and n = 1, `n ge 2` can't fail.
        assert_eq!(cov.uncovered, vec![(6, false)]);
        assert_eq!(cov.values, 4, "n = 0 and s = 0, 1, 2");
        assert!(cov.unproduced.is_empty(), "{:?}", cov.unproduced);
        let total = |r: &Vec<String>| -> Option<usize> {
            r.iter()
                .map(|c| match c.as_str() {
                    "0" => Some(0),
                    "1" => Some(1),
                    _ => None,
                })
                .sum()
        };
        let totals: HashSet<usize> = rows.iter().filter_map(total).collect();
        assert!(totals.contains(&0) && totals.contains(&1), "{rows:?}");
        assert!(totals.iter().any(|&n| n >= 2), "{rows:?}");
    }

    #[test]
    fn minimal_steps_counters_by_the_values_their_guards_list() {
        // Columns are a, b. `s = 0` needs both steps to add nothing.
        let (rows, cov) = minimal_rows(
            "if a in (0,1,2) and b in (0,1,2) then do;\n\
               s = 0;\n\
               if a in (0,1) then s = s + a; else if a = 2 then s = s + 3;\n\
               if b in (0,1,2) then s = s + b;\n\
             end;\n",
        );
        assert!(cov.unproduced.is_empty(), "{:?}", cov.unproduced);
        assert!(rows.iter().any(|r| r == &["0", "0"]), "{rows:?}");
    }

    #[test]
    fn max_comparisons_are_any_and_every_argument() {
        // Columns are a, b. Missing sorts below every number, so
        // `max(a, b) = 1` needs one argument at 1 and neither above it.
        let (rows, cov) = minimal_rows(
            "if a in (0:9) and b in (0:9) then do;\n\
               if max(a, b) ge 2 then x = 2;\n\
               else if max(a, b) = 1 then x = 1;\n\
               else x = 0;\n\
             end;\n",
        );
        assert_eq!(cov.values, 3);
        assert!(cov.unproduced.is_empty(), "{:?}", cov.unproduced);
        let max = |r: &Vec<String>| r.iter().filter_map(|c| c.parse::<i64>().ok()).max();
        assert!(rows.iter().any(|r| max(r) == Some(1)), "{rows:?}");
        assert!(rows.iter().any(|r| max(r) == Some(0)), "{rows:?}");
    }

    #[test]
    fn constant_recodes_decide_conditions_on_them() {
        // `h` is always 0: the guard on it holds, and `h = 2` never does,
        // so `s = 1` comes down to `a = 1`.
        let (rows, cov) = minimal_rows(
            "h = 0;\n\
             if a in (0, 1) and h in (0, 1) then do;\n\
               if h = 2 or a = 1 then s = 1; else s = 0;\n\
             end;\n",
        );
        assert!(cov.unproduced.is_empty(), "{:?}", cov.unproduced);
        assert!(
            rows.iter().any(|r| r == &["0"]) && rows.iter().any(|r| r == &["1"]),
            "{rows:?}"
        );
    }

    #[test]
    fn definitions_in_else_arms_of_outer_blocks_are_exclusive() {
        let atom = |var: &str, v: i64| {
            Pred::Atom(Atom {
                var: var.into(),
                op: Op::Eq,
                values: vec![Value::Int(v)],
                is_array: false,
            })
        };
        let def = |cond: Vec<Pred>| Def {
            var: "x".into(),
            value: DefValue::Const(Value::Int(1)),
            cond,
        };
        // if a = 1 then do; if b = 1 then x = …; end;
        // else if a = 2 then do; if c = 1 then x = …; end;
        let defs = vec![
            def(vec![atom("a", 1), atom("b", 1)]),
            def(vec![
                Pred::Not(Box::new(atom("a", 1))),
                atom("a", 2),
                atom("c", 1),
            ]),
            def(vec![atom("c", 1)]),
        ];
        assert!(exclusive(&defs, 0, 1));
        assert!(!exclusive(&defs, 0, 2));
        assert!(!exclusive(&defs, 1, 2));
    }

    #[test]
    fn minimal_steps_counters_by_capped_counters() {
        // Columns are a, b, c, d. `s = 0` needs `n` present and every step
        // off: `n = 1` and `n ge 2` both fail, which only `n = 0` does.
        let (rows, cov) = minimal_rows(
            "if a in (0,1) and b in (0,1) and c in (0,1) and d in (0,1) then do;\n\
               n = 0;\n\
               if a = 1 then n = n + 1;\n\
               if b = 1 then n = n + 1;\n\
               if c = 1 then n = n + 1;\n\
               s = 0;\n\
               if d = 1 then s = s + 1;\n\
               if n in (0,1) then s = s + n;\n\
               else if n ge 2 then s = s + 2;\n\
             end;\n",
        );
        assert!(cov.unproduced.is_empty(), "{:?}", cov.unproduced);
        assert!(rows.iter().any(|r| r == &["0", "0", "0", "0"]), "{rows:?}");
    }

    #[test]
    fn comparisons_on_one_count_merge_into_the_totals_all_accept() {
        let term = |var: &str| {
            Pred::Atom(Atom {
                var: var.into(),
                op: Op::Eq,
                values: vec![Value::Int(1)],
                is_array: false,
            })
        };
        let count = |op, v: i64| {
            Pred::Count(CountAtom {
                base: 0.0,
                terms: vec![(term("a"), 1.0), (term("b"), 1.0)],
                op,
                values: vec![Value::Int(v)],
            })
        };
        let merged = merge_counts(&[count(Op::Ne, 1), Pred::And(vec![count(Op::Lt, 2)])]);
        let [Pred::Count(c)] = &merged[..] else {
            panic!("{merged:?}")
        };
        assert_eq!((c.op, c.values.clone()), (Op::In, vec![Value::Float(0.0)]));
        assert_eq!(
            merge_counts(&[count(Op::Eq, 1), count(Op::Eq, 2)]),
            vec![Pred::Or(Vec::new())]
        );
    }

    #[test]
    fn minimal_reaches_each_value_of_sums_of_inputs() {
        // Columns are a, b. `7 - (a + b)` is missing when either is, so
        // `s = 0` needs both present and summing to 7.
        let (rows, cov) = minimal_rows(
            "if a in (0:4) and b in (0:4) then do;\n\
               x = 7 - (a + b);\n\
               if x gt 0 then s = 1; else if x = 0 then s = 0; else s = 2;\n\
             end;\n",
        );
        assert!(cov.unproduced.is_empty(), "{:?}", cov.unproduced);
        let sum = |r: &Vec<String>| r.iter().map(|c| c.parse::<f64>().ok()).sum::<Option<f64>>();
        assert!(rows.iter().any(|r| sum(r) == Some(7.0)), "{rows:?}");
        assert!(
            rows.iter().all(|r| !r.iter().any(|c| c.ends_with(".0"))),
            "{rows:?}"
        );
    }

    #[test]
    fn sums_of_comparisons_count_the_ones_that_hold() {
        // Columns are a, b, c. `(c = 0) = 0` is `c ne 0`.
        let (rows, cov) = minimal_rows(
            "if a in (0,1) and b in (0,1) and c in (0,1) then do;\n\
               if (c = 0) = 0 then n = (a = 0) + (b = 0);\n\
               else n = 9;\n\
               if n = 2 then s = 2; else if n = 1 then s = 1; else if n = 0 then s = 0;\n\
             end;\n",
        );
        assert!(cov.unproduced.is_empty(), "{:?}", cov.unproduced);
        assert!(rows.iter().any(|r| r == &["0", "0", "1"]), "{rows:?}");
    }

    #[test]
    fn linear_sums_parse_signs_through_parentheses() {
        let rhs = |src: &str| {
            let tokens = TokenStream::tokenize(src).default;
            let end = statement_end(&tokens, 0);
            tokens[..end].to_vec()
        };
        let (base, terms) = linear_sum("x", &rhs("7 - (a + b - 2) + (c = 0) - a;")).unwrap();
        assert_eq!(base, 9.0);
        let shown: Vec<(String, f64)> = terms
            .iter()
            .map(|(o, k)| match o {
                Operand::Var(v) => (v.clone(), *k),
                Operand::Test(r) => (format!("test{}", r.end() - r.start() + 1), *k),
            })
            .collect();
        assert_eq!(
            shown,
            vec![
                ("a".into(), -2.0),
                ("b".into(), -1.0),
                ("test3".into(), 1.0)
            ]
        );
        assert!(linear_sum("x", &rhs("x + 1;")).is_none());
        assert!(linear_sum("x", &rhs("a;")).is_none());
    }

    #[test]
    fn not_in_and_not_eq_are_negated_comparisons() {
        // Columns are a, b. With `not` parsed, the guard is fully known
        // and both values are reachable.
        let (rows, cov) = minimal_rows(
            "if a not in (5, 6) and b not eq 9 then do;\n\
               if a = 1 then s = 1; else s = 0;\n\
             end;\n",
        );
        assert!(cov.unproduced.is_empty(), "{:?}", cov.unproduced);
        let passes = |r: &Vec<String>| !["5", "6"].contains(&r[0].as_str()) && r[1] != "9";
        assert!(rows.iter().any(|r| passes(r) && r[0] != "1"), "{rows:?}");
    }

    #[test]
    fn sum_calls_skip_missing_arguments() {
        // Columns are a, b. `sum(a, b)` is 0 with one missing and the
        // other 0, and missing only when both are.
        let (rows, cov) = minimal_rows(
            "if a in (.,0,1) and b in (.,0,1) then do;\n\
               n = sum(a, b);\n\
               if n = 2 then s = 2; else if n = 1 then s = 1;\n\
               else if n = 0 then s = 0; else s = 9;\n\
             end;\n",
        );
        assert!(cov.unproduced.is_empty(), "{:?}", cov.unproduced);
        assert!(
            rows.iter().any(|r| r == &[".", "."] || r == &["", ""]),
            "{rows:?}"
        );
    }

    #[test]
    fn sums_of_maxima_reach_every_total() {
        // Columns are a, b, c. `max(of a b) + c` is 4 only at a or b = 2
        // and c = 2.
        let (rows, cov) = minimal_rows(
            "if a in (0,1,2) and b in (0,1,2) and c in (0,1,2) then do;\n\
               n = sum(max(of a b), c);\n\
               if n = 4 then s = 4; else if n = 0 then s = 0; else s = 1;\n\
             end;\n",
        );
        assert!(cov.unproduced.is_empty(), "{:?}", cov.unproduced);
        assert!(rows
            .iter()
            .any(|r| (r[0] == "2" || r[1] == "2") && r[2] == "2"));
    }

    #[test]
    fn definitions_needing_disjoint_values_are_exclusive() {
        let atom = |v: Vec<i64>| {
            Pred::Atom(Atom {
                var: "a".into(),
                op: Op::In,
                values: v.into_iter().map(Value::Int).collect(),
                is_array: false,
            })
        };
        let def = |cond: Vec<Pred>| Def {
            var: "x".into(),
            value: DefValue::Const(Value::Int(1)),
            cond,
        };
        let defs = vec![
            def(vec![Pred::And(vec![atom(vec![0])])]),
            def(vec![atom(vec![1, 2])]),
            def(vec![atom(vec![0, 1])]),
        ];
        assert!(exclusive(&defs, 0, 1));
        assert!(!exclusive(&defs, 0, 2));
        assert!(!exclusive(&defs, 1, 2));
    }

    #[test]
    fn declared_inputs_keep_their_value_until_assigned() {
        // `f` is mapped as an input but overridden when `g` says so.
        let g = graph("if g = 1 then f = 0;\nf = f;\nif f = 1 then x = 1;\n");
        assert_eq!(g.inputs, vec!["f", "g"]);
        let row = |f: i64, gv: i64| -> Row {
            [("f", f), ("g", gv)]
                .into_iter()
                .map(|(k, v)| (k.to_string(), Value::Int(v)))
                .collect()
        };
        let leaf = &g.branches[1].leaf;
        assert_eq!(eval(leaf, &row(1, 0), &g, None), Some(true));
        assert_eq!(eval(leaf, &row(1, 1), &g, None), Some(false));
    }

    #[test]
    fn count_picks_follow_atoms_already_fixed() {
        let eq1 = |var: &str| {
            Pred::Atom(Atom {
                var: var.to_string(),
                op: Op::Eq,
                values: vec![Value::Int(1)],
                is_array: false,
            })
        };
        let count = CountAtom {
            base: 0.0,
            terms: vec![(eq1("a"), 1.0), (eq1("b"), 1.0)],
            op: Op::Eq,
            values: vec![Value::Int(1)],
        };
        let Pred::Atom(fixed) = eq1("a") else {
            unreachable!()
        };
        for seed in 0..32 {
            let picked = pick_count(&count, std::slice::from_ref(&fixed), &mut Rng::new(seed));
            let Pred::And(c) = &picked else {
                panic!("{picked:?}")
            };
            assert_eq!(c[0], eq1("a"), "seed {seed}");
            assert_eq!(c[1], negate(&eq1("b")), "seed {seed}");
        }
    }

    #[test]
    fn counts_keep_one_arm_of_each_inputs_else_if_chain() {
        // A total of 0 needs every input's `+0` arm: skipping one leaves
        // its input no value the guard allows.
        let mut src =
            String::from("if a in (0,1,2) and b in (0,1,2) and c in (0,1,2) then do;\nn = 0;\n");
        for v in ["a", "b", "c"] {
            src += &format!(
                "if {v} = 0 then n = n + 0;\n\
                 else if {v} = 1 then n = n + 1;\n\
                 else if {v} = 2 then n = n + 2;\n"
            );
        }
        src += "if n = 0 then none = 1;\nend;\n";
        let (rows, cov) = minimal_rows(&src);
        assert!(cov.unproduced.is_empty(), "{:?}", cov.unproduced);
        assert!(rows.iter().any(|r| r.iter().all(|v| v == "0")));
    }

    #[test]
    fn sum_statements_step_counters() {
        // Columns are a, b.
        let (rows, cov) = minimal_rows(
            "n = 0;\n\
             if a in (4,5) then n + 1;\n\
             if b = 2 then n + 2;\n\
             if n >= 3 then hi = 1; else hi = 0;\n",
        );
        assert!(cov.uncovered.is_empty(), "{:?}", cov.uncovered);
        assert!(rows
            .iter()
            .any(|r| ["4", "5"].contains(&r[0].as_str()) && r[1] == "2"));
    }

    #[test]
    fn counters_reset_to_missing_stay_missing() {
        let g = graph("n = .;\nif a = 1 then do; n = 0; if b = 1 then n = n + 1; end;\nif n = . then x = 1;\n");
        let row: Row = [("a", 0), ("b", 1)]
            .into_iter()
            .map(|(k, v)| (k.to_string(), Value::Int(v)))
            .collect();
        let leaf = &g.branches.last().unwrap().leaf;
        assert_eq!(eval(leaf, &row, &g, None), Some(true));
    }

    #[test]
    fn conditional_copies_carry_conditions_to_their_source() {
        // Columns are k, v.
        let (rows, cov) = minimal_rows(
            "if k = 1 then do; xv = v; end;\n\
             else if k = 2 then do; xv = 0; end;\n\
             if xv in (5) then do; hit = 1; end;\n",
        );
        assert!(cov.uncovered.is_empty(), "{:?}", cov.uncovered);
        assert!(rows.iter().any(|r| r[0] == "1" && r[1] == "5"));
    }

    #[test]
    fn unit_conversions_carry_ranges_to_their_source() {
        // Columns are cm, inches; the second block overwrites the first.
        let (rows, cov) = minimal_rows(
            "m = cm / 100;\n\
             m = (inches * 0.0254);\n\
             if m > 1.22 and m < 2.28 then do; ok = 1; end;\n",
        );
        assert!(cov.uncovered.is_empty(), "{:?}", cov.uncovered);
        let m = |r: &Vec<String>| r[1].parse::<f64>().unwrap_or(0.0) * 0.0254;
        assert!(rows.iter().any(|r| m(r) > 1.22 && m(r) < 2.28));
        assert!(rows.iter().any(|r| !(m(r) > 1.22 && m(r) < 2.28)));
    }

    #[test]
    fn two_sided_ranges_are_satisfied_mid_range() {
        let (rows, cov) = minimal_rows("if x > 10 and x < 20 then do; ok = 1; end;\n");
        assert!(cov.uncovered.is_empty());
        assert!(rows.iter().any(|r| r[0] == "15"), "{rows:?}");
    }

    #[test]
    fn a_lone_dot_is_the_missing_value() {
        let g = graph("if a = . then x = 1;\nif b in (., 1) then y = 1;\nif c ne . then z = 1;\n");
        let missing = |p: &Pred, op: Op| matches!(p, Pred::Atom(a) if a.op == op && a.values.contains(&Value::Missing));
        assert!(missing(&g.predicates[0], Op::Eq));
        assert!(missing(&g.predicates[1], Op::In));
        assert!(missing(&g.predicates[2], Op::Ne));
    }

    #[test]
    fn variables_read_only_in_assignments_are_inputs() {
        let g = graph(
            "ht = raw_ht / 100;\n\
             wt = raw_wt;\n\
             if ht > 1 then bmi = wt / (ht * ht);\n\
             d = input(dob, yymmdd8.);\n\
             n = sum(of a1 a2);\n",
        );
        assert_eq!(g.inputs, vec!["a1", "a2", "dob", "raw_ht", "raw_wt"]);
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
    fn recode_copies_alias_predicates_to_their_inputs() {
        let src = "raw_smoke = raw_smoke;\nraw_drink = raw_drink;\n\
                   smoke = raw_smoke;\ndrink = raw_drink;\n\
                   if smoke in (0,1,2) and drink in (0,1,2,3) then do;\n\
                     if smoke in (1,2) or drink = 3 then flag = 1;\n\
                   end;\n";
        let g = graph(src);
        assert_eq!(g.inputs, vec!["raw_drink", "raw_smoke"]);
        let (rows, cov) = minimal_rows(src);
        assert_eq!((cov.covered(), cov.total, rows.len()), (4, 4, 3));
    }

    #[test]
    fn reassigned_or_conditional_copies_are_not_aliases() {
        let g = graph(
            "a = x;\nif x = 9 then a = 0;\n\
             if y = 1 then do; b = z; end;\n\
             if a = 1 and b = 2 then c = 1;\n",
        );
        // `z` is read and never assigned, so it is an input in its own
        // right; `b` is not an alias for it, so `b = 2` stays on `b`.
        assert_eq!(g.inputs, vec!["x", "y", "z"]);
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
