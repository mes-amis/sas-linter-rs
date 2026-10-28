use std::collections::BTreeMap;

use once_cell::sync::Lazy;
use regex::Regex;
use sas_lexer::{TokenChannel, TokenType};

use crate::config::opt_bool;
use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::{Token, TokenStream};

use super::{statement_not_valid, unterminated_comment, RuleMeta};

pub struct SourceHeaders {
    autofix: bool,
}

const ID: &str = "source_headers";
const DESCRIPTION: &str =
    "Header text that leaks out of its `**` comment and runs as code; will be re-wrapped.";

const TARGET_WIDTH: usize = 90;
const PAD_TO: usize = TARGET_WIDTH - 3; // leave 3 chars for trailing `**;`

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: true,
        default_factory: || Box::new(SourceHeaders { autofix: false }),
        config_factory: |opts| {
            Ok(Box::new(SourceHeaders {
                autofix: opt_bool(opts, "autofix").unwrap_or(false),
            }))
        },
    }
}

impl Rule for SourceHeaders {
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
        broken_lines(ctx.source, &ctx.tokens.all)
            .into_iter()
            .map(|(line_idx, why)| Finding {
                path: ctx.path.to_string(),
                line: (line_idx + 1) as u32,
                column: 1,
                rule: ID,
                message: why.message(),
                severity: Severity::Warning,
            })
            .collect()
    }

    fn autofix(&self, source: &str) -> String {
        // Re-wrapping a line can change how the lines after it lex, so
        // repeat until nothing is left to fix.
        let mut text = source.to_string();
        for _ in 0..10 {
            let stream = TokenStream::tokenize(&text);
            let bad = broken_lines(&text, &stream.all);
            if bad.is_empty() {
                break;
            }
            text = rewrite(&text, &bad);
        }
        text
    }
}

/// Why a header line needs re-wrapping.
#[derive(Clone, Copy)]
enum Broken {
    /// Header text that SAS reads as code: after a `;` inside the text, or
    /// a continuation line without its `**`.
    Leaks,
    /// A line of a header box with no `;`, so the box's comment runs on
    /// and swallows the code after it.
    Unterminated { code_line: u32 },
}

impl Broken {
    fn message(self) -> String {
        match self {
            Broken::Leaks => "header text leaks out of its `**` comment — SAS runs it as \
                              code."
                .into(),
            Broken::Unterminated { code_line } => format!(
                "header line has no `;`, so the header comment runs on and swallows \
                 the code on line {}.",
                code_line
            ),
        }
    }
}

/// 0-indexed header lines to re-wrap.
fn broken_lines(text: &str, tokens: &[Token]) -> BTreeMap<usize, Broken> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut bad = BTreeMap::new();
    for i in leaked_lines(&lines, tokens) {
        bad.insert(i, Broken::Leaks);
    }
    for s in unterminated_comment::swallows(tokens, text) {
        let first = (s.comment.start_line - 1) as usize;
        let code = (s.code_line - 1) as usize;
        let rows: Vec<usize> = (first..code)
            .filter(|&i| is_header_shaped(lines[i]))
            .collect();
        // One unterminated line is `unterminated_comment`'s; a box is ours.
        if rows.len() < 2 {
            continue;
        }
        for i in rows {
            if !lines[i].trim_end().ends_with(';') {
                bad.entry(i).or_insert(Broken::Unterminated {
                    code_line: s.code_line,
                });
            }
        }
    }
    bad
}

/// A `**` text line or a row of stars.
fn is_header_shaped(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("**") || RE_ALL_STARS.is_match(t)
}

/// Lines where code begins that is header text, not a statement: the code
/// starts a statement (the code before it ended with `;`), doesn't start
/// like any SAS statement, and sits on a `**` line or between `**` lines
/// of the header.
fn leaked_lines(lines: &[&str], tokens: &[Token]) -> Vec<usize> {
    let code: Vec<&Token> = tokens
        .iter()
        .filter(|t| t.channel == TokenChannel::DEFAULT && t.token_type != TokenType::EOF)
        .collect();
    let cutoff = code
        .iter()
        .find(|t| matches!(t.token_type, TokenType::KwData | TokenType::KwProc))
        .map_or(lines.len(), |t| (t.start_line - 1) as usize);

    let mut out = Vec::new();
    for (k, t) in code.iter().enumerate() {
        let prev = k.checked_sub(1).map(|p| code[p]);
        let starts_statement = prev.is_none_or(|p| p.token_type == TokenType::SEMI);
        let first_on_line = prev.is_none_or(|p| p.start_line != t.start_line);
        if !starts_statement || !first_on_line {
            continue;
        }
        let i = (t.start_line - 1) as usize;
        let stmt: Vec<Token> = code[k..]
            .iter()
            .take_while(|c| c.token_type != TokenType::SEMI)
            .map(|c| (*c).clone())
            .collect();
        // An empty statement (`;;`) is valid.
        if stmt.is_empty()
            || stmt.iter().any(statement_not_valid::is_macro)
            || statement_not_valid::starts_validly(&stmt)
        {
            continue;
        }
        let Some(line) = lines.get(i) else { continue };
        let in_header_box = i < cutoff
            && nearest_nonblank(lines, i as isize, -1)
                .is_some_and(|l| l.trim_start().starts_with("**"))
            && nearest_nonblank(lines, i as isize, 1)
                .is_some_and(|l| l.trim_start().starts_with("**"));
        if line.trim_start().starts_with("**") || in_header_box {
            out.push(i);
        }
    }
    out
}

fn nearest_nonblank<'a>(lines: &'a [&'a str], from: isize, step: isize) -> Option<&'a str> {
    let mut i = from + step;
    while i >= 0 && (i as usize) < lines.len() {
        let line = lines[i as usize];
        if !line.trim().is_empty() {
            return Some(line);
        }
        i += step;
    }
    None
}

static RE_ALL_STARS: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\*+$").unwrap());

fn rewrite(text: &str, bad: &BTreeMap<usize, Broken>) -> String {
    let mut out = Vec::new();
    for (i, line) in text.split('\n').enumerate() {
        if bad.contains_key(&i) {
            out.extend(rewrite_line(line));
        } else {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

static RE_SPLIT_MARKERS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+\*\*\s+").unwrap());

/// Re-wrap one header line as `**  text  …  **;` padded to the header
/// width. A `*` comment ends at its first `;`, so `;` inside the text
/// becomes `,`.
fn rewrite_line(line: &str) -> Vec<String> {
    let (line, cr) = match line.strip_suffix('\r') {
        Some(body) => (body, "\r"),
        None => (line, ""),
    };
    let expanded = line.replace('\t', "    ");
    let stripped = expanded.trim_end();
    if RE_ALL_STARS.is_match(stripped) {
        return vec![format!("{};{}", stripped, cr)];
    }

    // Continuation line missing `**` prefix.
    let prefixed = if stripped.starts_with("**") {
        stripped.to_string()
    } else {
        format!("**  {}", stripped.trim_start())
    };

    let trimmed = if let Some(rest) = prefixed.strip_suffix("**;") {
        rest.trim_end().to_string()
    } else if let Some(rest) = prefixed.strip_suffix(';') {
        rest.trim_end().to_string()
    } else {
        prefixed.clone()
    };

    let trimmed = trimmed.replace(';', ",");
    let segments: Vec<&str> = RE_SPLIT_MARKERS.split(&trimmed).collect();
    segments
        .iter()
        .enumerate()
        .map(|(idx, seg)| {
            let body = if idx == 0 {
                seg.trim_end().to_string()
            } else {
                format!("**  {}", seg.trim())
            };
            let body = if body.starts_with("**") {
                body
            } else {
                format!("**  {}", body)
            };
            let padded = if body.chars().count() < PAD_TO {
                let pad = PAD_TO - body.chars().count();
                format!("{}{}", body, " ".repeat(pad))
            } else {
                body
            };
            format!("{}**;{}", padded, cr)
        })
        .collect()
}
