use std::collections::HashSet;

use sas_lexer::TokenType;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::Token;

use super::RuleMeta;

pub struct FormatForUnknownVariable;

const ID: &str = "format_for_unknown_variable";
const DESCRIPTION: &str = "Variable named in a `format` / `informat` / `attrib` \
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
}

impl FormatKind {
    fn from_type(t: TokenType) -> Option<Self> {
        match t {
            TokenType::KwFormat => Some(FormatKind::Format),
            TokenType::KwInformat => Some(FormatKind::Informat),
            TokenType::KwAttrib => Some(FormatKind::Attrib),
            _ => None,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            FormatKind::Format => "format",
            FormatKind::Informat => "informat",
            FormatKind::Attrib => "attrib",
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
        let mut targets: Vec<(Token, FormatKind)> = Vec::new();
        let mut use_names: HashSet<String> = HashSet::new();

        for stmt in iter_statements(tokens) {
            if stmt.is_empty() {
                continue;
            }
            let opener = &stmt[0];
            if is_external_input(opener.token_type) {
                external_input = true;
                continue;
            }
            if let Some(kind) = FormatKind::from_type(opener.token_type) {
                collect_targets(stmt, kind, &mut targets);
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
            .filter_map(|(t, kind)| {
                if use_names.contains(&t.text.to_lowercase()) {
                    None
                } else {
                    Some(Finding {
                        path: ctx.path.to_string(),
                        line: t.start_line,
                        column: t.start_column + 1,
                        rule: ID,
                        message: format!(
                            "`{}` assigns a format to `{}` but that variable is not \
                             referenced anywhere else in this file — likely a typo.",
                            kind.as_str(),
                            t.text
                        ),
                        severity: Severity::Warning,
                    })
                }
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

fn collect_targets(stmt: &[Token], kind: FormatKind, out: &mut Vec<(Token, FormatKind)>) {
    for (i, t) in stmt.iter().enumerate() {
        if t.token_type != TokenType::Identifier {
            continue;
        }
        if !is_variable_target(stmt, i) {
            continue;
        }
        out.push((t.clone(), kind));
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

fn collect_uses(stmt: &[Token], names: &mut HashSet<String>) {
    for (i, t) in stmt.iter().enumerate() {
        if t.token_type != TokenType::Identifier {
            continue;
        }
        if !is_variable_use(stmt, i) {
            continue;
        }
        names.insert(t.text.to_lowercase());
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
