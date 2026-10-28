use std::ops::Range;

use sas_lexer::TokenType;

use crate::config::{opt_bool, opt_map_str_str};
use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Hook, Rule};
use crate::token::TokenStream;

use super::tab_expansion::literal_byte_ranges;
use super::RuleMeta;

pub struct EncodingIssues {
    use_defaults: bool,
    replacements: Vec<(String, String)>,
    autofix: bool,
}

const ID: &str = "encoding_issues";
const DESCRIPTION: &str = "Source contains smart-punctuation / Win-1252 byte sequences.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: true,
        default_factory: || {
            Box::new(EncodingIssues {
                use_defaults: false,
                replacements: Vec::new(),
                autofix: false,
            })
        },
        config_factory: |opts| {
            Ok(Box::new(EncodingIssues {
                use_defaults: opt_bool(opts, "use_defaults").unwrap_or(false),
                replacements: opt_map_str_str(opts, "replacements"),
                autofix: opt_bool(opts, "autofix").unwrap_or(false),
            }))
        },
    }
}

/// Smart punctuation folded to ASCII. Sources reach the rule as UTF-8:
/// `encoding::read_source` transcodes Win-1252 files first, so a raw 0x92
/// byte arrives here as U+2019 and is covered by this table.
fn default_replacement(ch: char) -> Option<&'static str> {
    Some(match ch {
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => "'",
        '\u{201C}' | '\u{201D}' | '\u{201E}' => "\"",
        '\u{2013}' | '\u{2014}' | '\u{2015}' => "-",
        '\u{2026}' => "...",
        '\u{00A0}' | '\u{2000}'..='\u{200A}' => " ",
        '\u{2028}' | '\u{2029}' => "\n",
        _ => return None,
    })
}

/// Mac Roman punctuation (– “ ” ‘ ’) read as Win-1252 / Latin-1 shows up
/// as Ð Ò Ó Ô Õ. Those are also real letters (`'ÓSCAR'`), so they are
/// reported for a human to judge but never rewritten; a project that knows
/// its files are Mac Roman can fold them with `replacements:`.
fn mac_roman_suspect(ch: char) -> Option<char> {
    Some(match ch {
        '\u{00D0}' => '\u{2013}',
        '\u{00D2}' => '\u{201C}',
        '\u{00D3}' => '\u{201D}',
        '\u{00D4}' => '\u{2018}',
        '\u{00D5}' => '\u{2019}',
        _ => return None,
    })
}

impl Rule for EncodingIssues {
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
    fn hook(&self) -> Hook {
        Hook::BeforeFormat
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        let mut findings = Vec::new();
        if self.use_defaults {
            findings.extend(self.default_findings(ctx));
        }
        if !self.replacements.is_empty() {
            findings.extend(replacement_findings(
                ctx.source,
                ctx.path,
                &self.replacements,
                self.autofix,
            ));
        }
        findings
    }

    fn autofix(&self, source: &str) -> String {
        let mut out = source.to_string();
        // User replacements run FIRST so project-specific patterns can
        // target sequences the canonical defaults would otherwise consume.
        // Each pass re-tokenizes: an earlier replacement can move or create
        // string literals.
        for (from, to) in &self.replacements {
            if from.is_empty() || !out.contains(from.as_str()) {
                continue;
            }
            let matches: Vec<_> = out
                .match_indices(from.as_str())
                .map(|(i, m)| (i..i + m.len(), to.as_str()))
                .collect();
            let edits = quote_safe_edits(&out, &TokenStream::tokenize(&out), matches);
            out = splice(&out, &edits);
        }
        if self.use_defaults {
            let edits = default_edits(&out, &TokenStream::tokenize(&out));
            out = splice(&out, &edits);
        }
        out
    }
}

impl EncodingIssues {
    fn default_findings(&self, ctx: &CheckContext) -> Vec<Finding> {
        let suffix = if self.autofix { " (autofixed)" } else { "" };
        let mut found: Vec<(usize, String)> = default_edits(ctx.source, ctx.tokens)
            .into_iter()
            .map(|e| {
                let ch = ctx.source[e.bytes.start..].chars().next().unwrap_or('\0');
                (
                    e.bytes.start,
                    format!("UTF-8 U+{:04X} -> {:?}{}", ch as u32, e.text, suffix),
                )
            })
            .collect();
        for (i, ch) in ctx.source.char_indices() {
            if let Some(meant) = mac_roman_suspect(ch) {
                found.push((
                    i,
                    format!(
                        "U+{:04X} {:?} may be Mac Roman mojibake for {:?} (no autofix)",
                        ch as u32, ch, meant
                    ),
                ));
            }
        }
        found.sort_by_key(|(i, _)| *i);
        found
            .into_iter()
            .map(|(i, message)| {
                let (line, column) = line_col(ctx.source, i);
                Finding {
                    path: ctx.path.to_string(),
                    line,
                    column,
                    rule: ID,
                    message,
                    severity: Severity::Warning,
                }
            })
            .collect()
    }
}

/// One rewrite: replace `bytes` of the source with `text`.
struct Edit {
    bytes: Range<usize>,
    text: String,
}

/// Every default-table character in `source`, as quote-safe edits.
fn default_edits(source: &str, tokens: &TokenStream) -> Vec<Edit> {
    let matches = source
        .char_indices()
        .filter_map(|(i, ch)| default_replacement(ch).map(|r| (i..i + ch.len_utf8(), r)))
        .collect();
    quote_safe_edits(source, tokens, matches)
}

/// Where a quote character produced by a replacement could change how SAS
/// splits the program into tokens.
#[derive(Clone, Copy, PartialEq)]
enum Region {
    /// Inside a string literal delimited by this quote character.
    Quoted(char),
    /// Inside a `*…;` or `%*…;` comment statement, where SAS pairs quotes.
    CommentStatement,
}

/// Sorted, non-overlapping regions of `source` that constrain quotes.
/// Datalines are deliberately absent: card images have no escaping.
fn quote_regions(source: &str, tokens: &TokenStream) -> Vec<(Range<usize>, Region)> {
    let datalines: Vec<Range<usize>> = tokens
        .all
        .iter()
        .filter(|t| t.token_type == TokenType::DatalinesData)
        .map(|t| t.start_byte as usize..t.end_byte as usize)
        .collect();
    let mut regions: Vec<(Range<usize>, Region)> = literal_byte_ranges(tokens)
        .into_iter()
        .filter(|r| !datalines.iter().any(|d| d.contains(&r.start)))
        .filter_map(|r| match source[r.start..].chars().next() {
            // The interior only: a match on the opening delimiter itself
            // is not "inside" the string.
            Some(q @ ('\'' | '"')) => Some((r.start + 1..r.end, Region::Quoted(q))),
            _ => None,
        })
        .collect();
    regions.extend(
        tokens
            .all
            .iter()
            .filter(|t| {
                matches!(
                    t.token_type,
                    TokenType::PredictedCommentStat | TokenType::MacroComment
                )
            })
            .map(|t| {
                (
                    t.start_byte as usize..t.end_byte as usize,
                    Region::CommentStatement,
                )
            }),
    );
    regions.sort_by_key(|(r, _)| r.start);
    regions
}

/// Turn raw `(bytes, replacement)` matches (sorted, non-overlapping) into
/// edits that keep the token structure intact:
///
/// * inside a string literal, a replacement character equal to the
///   delimiter is doubled — SAS's escape (`'It''s'`);
/// * inside a comment statement, quote characters are doubled when the
///   plainly-folded comment would leave a quote unmatched (SAS would read
///   on into the following code looking for its partner).
fn quote_safe_edits(
    source: &str,
    tokens: &TokenStream,
    matches: Vec<(Range<usize>, &str)>,
) -> Vec<Edit> {
    if matches.is_empty() {
        return Vec::new();
    }
    let regions = quote_regions(source, tokens);
    let region_of = |pos: usize| {
        let idx = regions.partition_point(|(r, _)| r.end <= pos);
        regions.get(idx).filter(|(r, _)| r.start <= pos).cloned()
    };

    let mut edits: Vec<Edit> = Vec::with_capacity(matches.len());
    let mut i = 0;
    while i < matches.len() {
        let (bytes, text) = &matches[i];
        match region_of(bytes.start) {
            Some((_, Region::Quoted(q))) => {
                let doubled: String = text
                    .chars()
                    .flat_map(|c| double_if(c, |c| c == q))
                    .collect();
                edits.push(Edit {
                    bytes: bytes.clone(),
                    text: doubled,
                });
                i += 1;
            }
            Some((comment, Region::CommentStatement)) => {
                // Decide for the whole comment at once.
                let group_end = matches[i..]
                    .iter()
                    .position(|(b, _)| b.start >= comment.end)
                    .map_or(matches.len(), |n| i + n);
                let group = &matches[i..group_end];
                let plain: Vec<Edit> = group
                    .iter()
                    .map(|(b, t)| Edit {
                        bytes: b.clone(),
                        text: t.to_string(),
                    })
                    .collect();
                let rel = comment.start;
                let folded = splice(&source[comment.clone()], &shift(&plain, rel));
                if quotes_balanced(&folded) {
                    edits.extend(plain);
                } else {
                    edits.extend(group.iter().map(|(b, t)| {
                        Edit {
                            bytes: b.clone(),
                            text: t
                                .chars()
                                .flat_map(|c| double_if(c, |c| c == '\'' || c == '"'))
                                .collect(),
                        }
                    }));
                }
                i = group_end;
            }
            None => {
                edits.push(Edit {
                    bytes: bytes.clone(),
                    text: text.to_string(),
                });
                i += 1;
            }
        }
    }
    edits
}

fn double_if(c: char, pred: impl Fn(char) -> bool) -> impl Iterator<Item = char> {
    std::iter::repeat_n(c, if pred(c) { 2 } else { 1 })
}

/// Rebase edits onto a slice that starts at byte `offset`.
fn shift(edits: &[Edit], offset: usize) -> Vec<Edit> {
    edits
        .iter()
        .map(|e| Edit {
            bytes: e.bytes.start - offset..e.bytes.end - offset,
            text: e.text.clone(),
        })
        .collect()
}

/// SAS's view: a quote opens a literal that runs to the next quote of the
/// same kind (a doubled quote closes and reopens, which still balances).
fn quotes_balanced(text: &str) -> bool {
    let mut open: Option<char> = None;
    for c in text.chars() {
        match open {
            Some(q) if c == q => open = None,
            None if c == '\'' || c == '"' => open = Some(c),
            _ => {}
        }
    }
    open.is_none()
}

/// Apply sorted, non-overlapping edits.
fn splice(source: &str, edits: &[Edit]) -> String {
    let mut out = String::with_capacity(source.len());
    let mut copied = 0;
    for e in edits {
        out.push_str(&source[copied..e.bytes.start]);
        out.push_str(&e.text);
        copied = e.bytes.end;
    }
    out.push_str(&source[copied..]);
    out
}

/// 1-based line and 1-based character column of byte offset `pos`.
fn line_col(source: &str, pos: usize) -> (u32, u32) {
    let before = &source[..pos];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |n| n + 1);
    let column = source[line_start..pos].chars().count() + 1;
    (line as u32, column as u32)
}

fn replacement_findings(
    source: &str,
    path: &str,
    replacements: &[(String, String)],
    autofix: bool,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (line_idx, line) in source.split_inclusive('\n').enumerate() {
        let chomped = line.trim_end_matches('\n').trim_end_matches('\r');
        for (from, to) in replacements {
            if from.is_empty() {
                continue;
            }
            for (abs, _) in chomped.match_indices(from.as_str()) {
                let col = chomped[..abs].chars().count() + 1;
                findings.push(Finding {
                    path: path.to_string(),
                    line: (line_idx + 1) as u32,
                    column: col as u32,
                    rule: ID,
                    message: if autofix {
                        format!("found {:?} -> {:?} (autofixed)", from, to)
                    } else {
                        format!("found {:?} (no autofix)", from)
                    },
                    severity: Severity::Warning,
                });
            }
        }
    }
    findings
}
