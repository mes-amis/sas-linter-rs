use std::collections::HashSet;

use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::Token;

use super::RuleMeta;

pub struct FormatForUnknownVariable;

const ID: &str = "format_for_unknown_variable";
const DESCRIPTION: &str = "Variable named in a `format` / `informat` / `attrib` / `label` \
                            statement is referenced nowhere else in the file — \
                            almost always a typo.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: false,
        default_factory: || Box::new(FormatForUnknownVariable),
        config_factory: |_| Ok(Box::new(FormatForUnknownVariable)),
    }
}

#[derive(Copy, Clone, Debug)]
enum FormatKind {
    Format,
    Informat,
    Attrib,
    Label,
}

impl FormatKind {
    fn from_type(t: TokenType) -> Option<Self> {
        match t {
            TokenType::KwFormat => Some(FormatKind::Format),
            TokenType::KwInformat => Some(FormatKind::Informat),
            TokenType::KwAttrib => Some(FormatKind::Attrib),
            TokenType::KwLabel => Some(FormatKind::Label),
            _ => None,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            FormatKind::Format => "format",
            FormatKind::Informat => "informat",
            FormatKind::Attrib => "attrib",
            FormatKind::Label => "label",
        }
    }
    fn assigns(self) -> &'static str {
        match self {
            FormatKind::Label => "a label",
            _ => "a format",
        }
    }
}

fn is_external_input(t: TokenType) -> bool {
    matches!(
        t,
        TokenType::KwSet
            | TokenType::KwMerge
            | TokenType::KwUpdate
            | TokenType::KwInfile
            | TokenType::KwInput
    )
}

fn is_declaration_opener(t: &Token) -> bool {
    matches!(
        t.token_type,
        TokenType::KwFormat
            | TokenType::KwInformat
            | TokenType::KwAttrib
            | TokenType::KwLabel
            | TokenType::KwLength
            | TokenType::KwKeep
            | TokenType::KwDrop
            | TokenType::KwArray
    ) || (t.token_type == TokenType::Identifier
        && matches!(
            t.text.to_lowercase().as_str(),
            "retain" | "value" | "invalue" | "picture"
        ))
}

impl Rule for FormatForUnknownVariable {
    fn id(&self) -> &'static str {
        ID
    }
    fn description(&self) -> &'static str {
        DESCRIPTION
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        let tokens = &ctx.tokens.default;
        let mut external_input = false;
        let mut targets: Vec<Target> = Vec::new();
        let mut use_names: HashSet<String> = HashSet::new();
        // PROC statements name the input dataset's variables, which the file
        // needn't mention anywhere else. A PROC runs until the next step.
        let mut in_proc = false;

        for stmt in iter_statements(tokens) {
            if stmt.is_empty() {
                continue;
            }
            let opener = &stmt[0];
            match opener.token_type {
                TokenType::KwProc => in_proc = true,
                TokenType::KwData => in_proc = false,
                _ => {}
            }
            if is_external_input(opener.token_type) {
                external_input = true;
                continue;
            }
            if let Some(kind) = FormatKind::from_type(opener.token_type) {
                if !in_proc {
                    collect_targets(stmt, kind, &mut targets);
                }
                continue;
            }
            if opener.token_type == TokenType::KwArray {
                // The elements are used through the array (`a{i}`), however
                // they are spelled elsewhere. Skip the array's own name.
                collect_uses(stmt.get(2..).unwrap_or_default(), &mut use_names);
                continue;
            }
            if is_declaration_opener(opener) {
                continue;
            }
            collect_uses(stmt, &mut use_names);
        }

        if external_input {
            return Vec::new();
        }

        targets
            .into_iter()
            .filter(|t| !t.names.iter().any(|n| use_names.contains(n)))
            .map(|t| Finding {
                path: ctx.path.to_string(),
                line: t.token.start_line,
                column: t.token.start_column + 1,
                rule: ID,
                message: format!(
                    "`{}` assigns {} to `{}` but {} referenced anywhere else in \
                     this file — likely a typo.",
                    t.kind.as_str(),
                    t.kind.assigns(),
                    t.display,
                    if t.names.len() > 1 {
                        "none of those variables is"
                    } else {
                        "that variable is not"
                    }
                ),
                severity: Severity::Warning,
            })
            .collect()
    }
}

fn iter_statements(tokens: &[Token]) -> Vec<&[Token]> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, t) in tokens.iter().enumerate() {
        if t.token_type == TokenType::SEMI {
            let slice = &tokens[start..i];
            if !slice.is_empty() {
                out.push(slice);
            }
            start = i + 1;
        }
    }
    out
}

/// A variable — or a numbered range of them — named by a declaration.
struct Target {
    /// First token, where the finding is reported.
    token: Token,
    /// As written: `x`, or `x1-x3` for a numbered range.
    display: String,
    /// Lowercased names covered; the target is known if any one is used.
    names: Vec<String>,
    kind: FormatKind,
}

/// Words that make `a-numeric-c` / `a-character-c` / `a-all-c` a name range
/// list rather than two variables and a third called `numeric`.
const NAME_RANGE_WORDS: &[&str] = &["numeric", "character", "all"];

/// A numbered range list `x1-x3` → `x1`, `x2`, `x3`: both ends share a
/// prefix and end in a number, the first no larger than the last. Members
/// are zero-padded when both ends are written to the same width (`x01-x10`).
fn numbered_range(first: &str, last: &str) -> Option<Vec<String>> {
    /// Ranges wider than this are not expanded (only the endpoints count).
    const MAX_MEMBERS: u64 = 10_000;
    let split = |s: &str| {
        let digits = s.len() - s.trim_end_matches(|c: char| c.is_ascii_digit()).len();
        let (prefix, num) = s.split_at(s.len() - digits);
        (!prefix.is_empty() && digits > 0).then(|| (prefix.to_lowercase(), num.to_string()))
    };
    let (p1, n1) = split(first)?;
    let (p2, n2) = split(last)?;
    let (lo, hi) = (n1.parse::<u64>().ok()?, n2.parse::<u64>().ok()?);
    if p1 != p2 || lo > hi || hi - lo >= MAX_MEMBERS {
        return None;
    }
    let width = if n1.len() == n2.len() { n1.len() } else { 0 };
    Some((lo..=hi).map(|n| format!("{p1}{n:0width$}")).collect())
}

/// Recognize a variable list starting at `stmt[i]` (an identifier). Returns
/// the names it covers and how many tokens it spans. `a--c` and
/// `a-numeric-c` cover unknown positional spans, so only their endpoints
/// are names; `a1-a3` covers every member.
fn variable_list(stmt: &[Token], i: usize) -> (Vec<String>, usize) {
    let ty = |k: usize| stmt.get(k).map(|t| t.token_type);
    let ident = |k: usize| ty(k) == Some(TokenType::Identifier);
    let lower = |k: usize| stmt[k].text.to_lowercase();
    if ty(i + 1) == Some(TokenType::MINUS) {
        if ty(i + 2) == Some(TokenType::MINUS) && ident(i + 3) {
            return (vec![lower(i), lower(i + 3)], 4);
        }
        if ident(i + 2)
            && NAME_RANGE_WORDS.contains(&lower(i + 2).as_str())
            && ty(i + 3) == Some(TokenType::MINUS)
            && ident(i + 4)
        {
            return (vec![lower(i), lower(i + 4)], 5);
        }
        if ident(i + 2) {
            if let Some(members) = numbered_range(&stmt[i].text, &stmt[i + 2].text) {
                return (members, 3);
            }
        }
    }
    (vec![lower(i)], 1)
}

fn collect_targets(stmt: &[Token], kind: FormatKind, out: &mut Vec<Target>) {
    let mut i = 0;
    while i < stmt.len() {
        let t = &stmt[i];
        if t.token_type != TokenType::Identifier || !is_variable_target(stmt, i) {
            i += 1;
            continue;
        }
        let (names, span) = variable_list(stmt, i);
        if span == 3 {
            // Numbered range: one target, known if any member is used.
            out.push(Target {
                token: t.clone(),
                display: format!("{}-{}", t.text, stmt[i + 2].text),
                names,
                kind,
            });
        } else {
            // Single name, or the endpoints of a positional span.
            for k in (i..i + span).filter(|&k| stmt[k].token_type == TokenType::Identifier) {
                let name = stmt[k].text.to_lowercase();
                if names.contains(&name) {
                    out.push(Target {
                        token: stmt[k].clone(),
                        display: stmt[k].text.clone(),
                        names: vec![name],
                        kind,
                    });
                }
            }
        }
        i += span;
    }
}

fn is_variable_target(stmt: &[Token], i: usize) -> bool {
    let t = &stmt[i];
    let nxt = stmt.get(i + 1);
    let prev = i.checked_sub(1).and_then(|p| stmt.get(p));

    if let Some(n) = nxt {
        if n.token_type == TokenType::DOT && n.start_byte == t.end_byte {
            return false;
        }
    }
    if let Some(p) = prev {
        if p.token_type == TokenType::ASSIGN {
            return false;
        }
    }
    true
}

/// Expanding `x1-x3` in an expression where it is really a subtraction only
/// adds names to the known set — it can hide a finding, never invent one.
fn collect_uses(stmt: &[Token], names: &mut HashSet<String>) {
    let mut i = 0;
    while i < stmt.len() {
        if stmt[i].token_type != TokenType::Identifier || !is_variable_use(stmt, i) {
            i += 1;
            continue;
        }
        let (covered, span) = variable_list(stmt, i);
        names.extend(covered);
        i += span;
    }
}

fn is_variable_use(stmt: &[Token], i: usize) -> bool {
    let t = &stmt[i];
    let nxt = stmt.get(i + 1);
    let prev = i.checked_sub(1).and_then(|p| stmt.get(p));

    if let Some(n) = nxt {
        if n.token_type == TokenType::DOT && n.start_byte == t.end_byte {
            return false;
        }
    }
    if let Some(p) = prev {
        if p.token_type == TokenType::DOT && p.end_byte == t.start_byte {
            return false;
        }
    }
    true
}
