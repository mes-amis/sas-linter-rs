use std::collections::HashSet;

use once_cell::sync::Lazy;
use regex::Regex;
use sas_lexer::{TokenChannel, TokenType};

use crate::config::opt_bool;
use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::token::{Token, TokenStream};

use super::RuleMeta;

pub struct SourceHeaders {
    autofix: bool,
}

const ID: &str = "source_headers";
const DESCRIPTION: &str =
    "Header lines look like `**`-comments but lex as code; will be re-wrapped.";

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
        let bad = broken_header_lines(ctx.source);
        bad.into_iter()
            .map(|line_idx| Finding {
                path: ctx.path.to_string(),
                line: (line_idx + 1) as u32,
                column: 1,
                rule: ID,
                message: if self.autofix {
                    "broken header line (autofixed)".into()
                } else {
                    "broken header line".into()
                },
                severity: Severity::Warning,
            })
            .collect()
    }

    fn autofix(&self, source: &str) -> String {
        // Step 0: expand tabs to 4 spaces (Word documents).
        let mut text = source.replace('\t', "    ");
        for _ in 0..10 {
            let stream = TokenStream::tokenize(&text);
            let skip = c_comment_lines(&stream.all);
            let mut bad = broken_lines_for(&text, &stream.all, &skip);
            for ln in asterisk_rows_missing_semi(&text, &skip) {
                bad.insert(ln);
            }
            if bad.is_empty() {
                break;
            }
            text = rewrite(&text, &bad);
        }
        text
    }
}

/// Public-style helper for the `check` side: bad header line indices
/// without the iterative autofix loop.
fn broken_header_lines(source: &str) -> Vec<usize> {
    let stream = TokenStream::tokenize(source);
    let skip = c_comment_lines(&stream.all);
    let set = broken_lines_for(source, &stream.all, &skip);
    let mut v: Vec<usize> = set.into_iter().collect();
    v.sort();
    v
}

fn c_comment_lines(tokens: &[Token]) -> HashSet<usize> {
    let mut set = HashSet::new();
    for t in tokens {
        if t.token_type != TokenType::CStyleComment {
            continue;
        }
        for ln in
            (t.start_line as usize).saturating_sub(1)..=(t.end_line as usize).saturating_sub(1)
        {
            set.insert(ln);
        }
    }
    set
}

fn header_cutoff_line(tokens: &[Token], total_lines: usize) -> usize {
    for t in tokens {
        if matches!(t.token_type, TokenType::KwData | TokenType::KwProc) {
            return (t.start_line as usize).saturating_sub(1);
        }
    }
    total_lines
}

fn prose_only_line(tokens: &[Token], line_idx: usize) -> bool {
    let mut saw_default = false;
    for t in tokens {
        if (t.start_line as usize).saturating_sub(1) != line_idx {
            continue;
        }
        if t.channel != TokenChannel::DEFAULT {
            continue;
        }
        saw_default = true;
        if matches!(t.token_type, TokenType::SEMI | TokenType::ASSIGN) {
            return false;
        }
    }
    saw_default
}

fn broken_lines_for(text: &str, tokens: &[Token], skip: &HashSet<usize>) -> HashSet<usize> {
    let lines: Vec<&str> = text.split('\n').collect();
    let cutoff = header_cutoff_line(tokens, lines.len());

    let mut bad = HashSet::new();
    let mut default_lines: HashSet<usize> = HashSet::new();
    for t in tokens {
        if t.channel == TokenChannel::DEFAULT && t.token_type == TokenType::Identifier {
            default_lines.insert((t.start_line as usize).saturating_sub(1));
        }
    }

    for &i in &default_lines {
        if skip.contains(&i) {
            continue;
        }
        let Some(line) = lines.get(i) else { continue };
        if line.trim_start().starts_with("**") {
            // Properly terminated `**...**;` — don't re-pad; upstream
            // problem belongs to a different finding.
            if line.trim_end().ends_with("**;") {
                continue;
            }
            bad.insert(i);
        } else if i < cutoff {
            let prev = nearest_nonblank(&lines, i as isize, -1);
            let nxt = nearest_nonblank(&lines, i as isize, 1);
            let both_marker = prev
                .map(|s| s.trim_start().starts_with("**"))
                .unwrap_or(false)
                && nxt
                    .map(|s| s.trim_start().starts_with("**"))
                    .unwrap_or(false);
            if both_marker && prose_only_line(tokens, i) {
                bad.insert(i);
            }
        }
    }
    bad
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

fn asterisk_rows_missing_semi(text: &str, skip: &HashSet<usize>) -> HashSet<usize> {
    let mut bad = HashSet::new();
    for (i, line) in text.split('\n').enumerate() {
        if skip.contains(&i) {
            continue;
        }
        let trimmed = line.trim();
        if RE_ALL_STARS.is_match(trimmed) && !line.trim_end().ends_with(';') {
            bad.insert(i);
        }
    }
    bad
}

fn rewrite(text: &str, bad: &HashSet<usize>) -> String {
    let mut out = Vec::new();
    for (i, line) in text.split('\n').enumerate() {
        if bad.contains(&i) {
            for rewritten in rewrite_line(line) {
                out.push(rewritten);
            }
        } else {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

static RE_SPLIT_MARKERS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+\*\*\s+").unwrap());

fn rewrite_line(line: &str) -> Vec<String> {
    let stripped = line.trim_end();
    if RE_ALL_STARS.is_match(stripped) {
        return vec![format!("{};", stripped)];
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
            format!("{}**;", padded)
        })
        .collect()
}
