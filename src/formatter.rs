//! Source-level formatter — keyword casing, binary-operator spacing,
//! and indentation. The token passes mirror the Ruby `SasLinter::Formatter`
//! line-for-line: same config knobs (`keywords` / `operator_spacing` /
//! `indent_width`), same "preserve user line breaks" rule, same DATA /
//! PROC / DO / END / RUN / QUIT indentation state machine.
//!
//! On top of that (a deliberate divergence from the Ruby gem) two
//! opinionated alignment passes run by default — banner-comment box
//! alignment and assignment-run alignment (`src/align.rs`) — so that
//! `sas-lint --format` with no config is a one-stop cleanup in the
//! spirit of `terraform fmt`. Both can be disabled in the config.

use anyhow::{anyhow, Result};
use sas_lexer::{TokenChannel, TokenType};

use crate::config::Config;
use crate::token::{Token, TokenStream};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Casing {
    Preserve,
    Upper,
    Lower,
}

#[derive(Debug, Clone)]
pub struct Formatter {
    pub keywords: Casing,
    pub operator_spacing: bool,
    pub indent_width: Option<usize>,
    pub align_comment_banners: bool,
    pub align_assignments: bool,
}

impl Default for Formatter {
    fn default() -> Self {
        Formatter {
            keywords: Casing::Preserve,
            operator_spacing: false,
            indent_width: None,
            align_comment_banners: true,
            align_assignments: true,
        }
    }
}

impl Formatter {
    /// Build from the parsed `[format]` block in the YAML config.
    /// `keywords` must be one of `preserve` / `upper` / `lower`;
    /// `indent_width` defaults to disabled when omitted or ≤ 0.
    pub fn from_config(config: &Config) -> Result<Self> {
        let keywords = match config.format.keywords.as_deref().unwrap_or("preserve") {
            "preserve" => Casing::Preserve,
            "upper" => Casing::Upper,
            "lower" => Casing::Lower,
            other => {
                return Err(anyhow!(
                    "format.keywords must be 'preserve', 'upper', or 'lower' (got '{}')",
                    other
                ))
            }
        };
        let operator_spacing = config.format.operator_spacing.unwrap_or(false);
        let indent_width =
            config
                .format
                .indent_width
                .and_then(|w| if w > 0 { Some(w as usize) } else { None });
        let align_comment_banners = config.format.align_comment_banners.unwrap_or(true);
        let align_assignments = config.format.align_assignments.unwrap_or(true);
        Ok(Formatter {
            keywords,
            operator_spacing,
            indent_width,
            align_comment_banners,
            align_assignments,
        })
    }

    /// True when a pass that needs the token stream is enabled; the
    /// alignment passes are line-based and tokenize on their own.
    fn token_passes_enabled(&self) -> bool {
        self.keywords != Casing::Preserve || self.operator_spacing || self.indent_width.is_some()
    }

    pub fn format(&self, source: &str) -> String {
        let mut out = if self.token_passes_enabled() {
            let stream = TokenStream::tokenize(source);
            let reconstructed = self.reconstruct_owned(&stream.all);
            match self.indent_width {
                Some(w) => self.apply_indentation(&reconstructed, &stream.all, w),
                None => reconstructed,
            }
        } else {
            source.to_string()
        };
        if self.align_comment_banners {
            out = crate::align::align_comment_banners(&out);
        }
        if self.align_assignments {
            out = crate::align::align_assignments(&out);
        }
        out
    }

    fn apply_casing(&self, t: &Token) -> String {
        if self.keywords == Casing::Preserve {
            return t.text.clone();
        }
        if t.channel != TokenChannel::DEFAULT {
            return t.text.clone();
        }
        if !is_keyword_type(t.token_type) {
            return t.text.clone();
        }
        match self.keywords {
            Casing::Upper => t.text.to_uppercase(),
            Casing::Lower => t.text.to_lowercase(),
            Casing::Preserve => t.text.clone(),
        }
    }

    /// Re-indent each line per the computed level map. Lines with no
    /// token coverage (truly blank lines) are left alone except for
    /// trailing-whitespace trimming.
    fn apply_indentation(&self, source: &str, all_tokens: &[Token], width: usize) -> String {
        let levels = compute_line_levels(all_tokens);
        let mut out = String::new();
        for (idx, raw_line) in source.split_inclusive('\n').enumerate() {
            let lineno = (idx + 1) as u32;
            let (body, term) = if let Some(rest) = raw_line.strip_suffix("\r\n") {
                (rest, "\r\n")
            } else if let Some(rest) = raw_line.strip_suffix('\n') {
                (rest, "\n")
            } else {
                (raw_line, "")
            };
            let level = match levels.iter().find(|(ln, _)| *ln == lineno) {
                Some((_, lv)) => *lv,
                None => {
                    out.push_str(body);
                    out.push_str(term);
                    continue;
                }
            };
            let stripped = body.trim_start_matches([' ', '\t']);
            if stripped.is_empty() {
                out.push_str(term);
                continue;
            }
            for _ in 0..(width * level as usize) {
                out.push(' ');
            }
            out.push_str(stripped);
            out.push_str(term);
        }
        out
    }
}

/// Partition all_tokens into segments where each `tok` is a DEFAULT
/// token and `gap` is the indices of the HIDDEN/COMMENT tokens that
/// preceded it. A trailing gap (no following default token) is emitted
/// with `tok = None`.
#[derive(Debug, Clone)]
struct OwnedSegment {
    gap: Vec<usize>,
    tok: Option<usize>,
}

fn segmentize_owned(all_tokens: &[Token]) -> Vec<OwnedSegment> {
    let mut segs = Vec::new();
    let mut gap: Vec<usize> = Vec::new();
    for (i, t) in all_tokens.iter().enumerate() {
        if t.channel == TokenChannel::DEFAULT {
            segs.push(OwnedSegment {
                gap: std::mem::take(&mut gap),
                tok: Some(i),
            });
        } else {
            gap.push(i);
        }
    }
    if !gap.is_empty() {
        segs.push(OwnedSegment { gap, tok: None });
    }
    segs
}

impl Formatter {
    fn reconstruct_owned(&self, all_tokens: &[Token]) -> String {
        let segments = segmentize_owned(all_tokens);
        let mut out = String::new();
        for (idx, seg) in segments.iter().enumerate() {
            let prev_prev = if idx > 1 { segments[idx - 2].tok } else { None };
            let prev = if idx > 0 { segments[idx - 1].tok } else { None };
            let cur = seg.tok;
            let gap_text: String = seg
                .gap
                .iter()
                .map(|i| all_tokens[*i].text.as_str())
                .collect();

            if self.operator_spacing && prev.is_some() && cur.is_some() && !gap_text.contains('\n')
            {
                let desired = gap_desired(
                    prev_prev.map(|i| &all_tokens[i]),
                    prev.map(|i| &all_tokens[i]),
                    cur.map(|i| &all_tokens[i]),
                );
                match desired {
                    Some(s) => out.push_str(s),
                    None => out.push_str(&gap_text),
                }
            } else {
                out.push_str(&gap_text);
            }
            if let Some(ci) = cur {
                out.push_str(&self.apply_casing(&all_tokens[ci]));
            }
        }
        out
    }
}

fn is_keyword_type(t: TokenType) -> bool {
    // The Ruby version filters by name prefix (`KW_` / `KWM_`); the Rust
    // crate uses CamelCase variants `Kw…` / `Kwm…`. Debug format gives us
    // the variant name directly, which lets us avoid maintaining a long
    // hand-written list that would silently fall behind crate updates.
    let name = format!("{:?}", t);
    name.starts_with("Kw")
}

/// Returns the desired spacing between two same-line DEFAULT tokens:
///   None    → leave the gap unchanged
///   Some("") → no space
///   Some(" ") → exactly one space
///
/// `prev_prev` is needed so a unary `+`/`-` (PLUS/MINUS preceded by an
/// operator or by nothing) can be told from binary `+`/`-` (preceded by
/// a value-ending token).
fn gap_desired(
    prev_prev: Option<&Token>,
    prev: Option<&Token>,
    next: Option<&Token>,
) -> Option<&'static str> {
    let prev = prev?;
    let next = next?;
    let pt = prev.token_type;
    let nt = next.token_type;
    let ppt = prev_prev.map(|t| t.token_type);

    if NO_SPACE_BEFORE.contains(&nt) {
        return Some("");
    }
    if pt == TokenType::COMMA {
        return Some(" ");
    }

    if BINARY_OPS.contains(&pt) {
        if is_unary_candidate(pt) && (ppt.is_none() || !VALUE_ENDING.contains(&ppt.unwrap())) {
            return None;
        }
        return Some(" ");
    }
    if BINARY_OPS.contains(&nt) {
        if is_unary_candidate(nt) && !VALUE_ENDING.contains(&pt) {
            return None;
        }
        return Some(" ");
    }
    None
}

fn is_unary_candidate(t: TokenType) -> bool {
    matches!(t, TokenType::PLUS | TokenType::MINUS)
}

const BINARY_OPS: &[TokenType] = &[
    TokenType::ASSIGN,
    TokenType::PLUS,
    TokenType::MINUS,
    TokenType::STAR,
    TokenType::FSLASH,
    TokenType::STAR2,
    TokenType::LT,
    TokenType::LE,
    TokenType::GT,
    TokenType::GE,
    TokenType::NE,
    TokenType::LTGT,
    TokenType::GTLT,
    TokenType::AMP,
    TokenType::PIPE,
    TokenType::PIPE2,
    TokenType::EXCL,
    TokenType::EXCL2,
    TokenType::BPIPE,
    TokenType::BPIPE2,
    TokenType::SoundsLike,
];

const NO_SPACE_BEFORE: &[TokenType] = &[
    TokenType::SEMI,
    TokenType::COMMA,
    TokenType::RPAREN,
    TokenType::RBRACK,
];

const VALUE_ENDING: &[TokenType] = &[
    TokenType::Identifier,
    TokenType::IntegerLiteral,
    TokenType::FloatLiteral,
    TokenType::FloatExponentLiteral,
    TokenType::StringLiteral,
    TokenType::NameLiteral,
    TokenType::DateLiteral,
    TokenType::TimeLiteral,
    TokenType::DateTimeLiteral,
    TokenType::HexStringLiteral,
    TokenType::BitTestingLiteral,
    TokenType::MacroVarResolve,
    TokenType::MacroIdentifier,
    TokenType::StringExprEnd,
    TokenType::BitTestingLiteralExprEnd,
    TokenType::DateLiteralExprEnd,
    TokenType::DateTimeLiteralExprEnd,
    TokenType::HexStringLiteralExprEnd,
    TokenType::NameLiteralExprEnd,
    TokenType::TimeLiteralExprEnd,
    TokenType::RPAREN,
    TokenType::RBRACK,
];

/// Walk all_tokens and assign an indent level to each source line.
/// Only the FIRST token on a given line determines its level.
///
/// Nesting rules (verbatim from the Ruby version):
///   DATA / PROC → level 0; content after their SEMI → level 1
///   DO          → content inside indented one further level
///   END         → decrements level before assigning the END line's level
///   RUN / QUIT  → resets to level 0
fn compute_line_levels(all_tokens: &[Token]) -> Vec<(u32, u32)> {
    let mut levels: Vec<(u32, u32)> = Vec::new();
    let mut level: i32 = 0;
    let mut after_data_proc = false;

    let record = |levels: &mut Vec<(u32, u32)>, line: u32, lv: i32| {
        if !levels.iter().any(|(l, _)| *l == line) {
            levels.push((line, lv.max(0) as u32));
        }
    };

    for tok in all_tokens {
        if tok.channel == TokenChannel::HIDDEN {
            continue;
        }
        let line = tok.start_line;
        if tok.channel != TokenChannel::DEFAULT {
            // Comment token: indent at current level
            record(&mut levels, line, level);
            continue;
        }
        let ty = tok.token_type;
        if matches!(ty, TokenType::KwData | TokenType::KwProc) {
            level = 0;
            after_data_proc = true;
            record(&mut levels, line, level);
        } else if matches!(ty, TokenType::KwRun | TokenType::KwQuit) {
            level = 0;
            after_data_proc = false;
            record(&mut levels, line, level);
        } else if ty == TokenType::KwDo {
            record(&mut levels, line, level);
            level += 1;
        } else if ty == TokenType::KwEnd {
            level = (level - 1).max(0);
            record(&mut levels, line, level);
        } else if ty == TokenType::SEMI && after_data_proc {
            after_data_proc = false;
            level = 1;
        } else {
            record(&mut levels, line, level);
        }
    }
    levels
}
