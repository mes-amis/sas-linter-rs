//! Opinionated line-based alignment passes for `--format` mode.
//!
//! These go beyond the Ruby formatter's token passes. InterRAI-style
//! sources carry a banner preamble of `**  …  **;` comment lines whose
//! columns were once tab-aligned; after tab expansion at the wrong stop
//! width nothing lines up and every `**;` terminator lands at a
//! different column. In the same files, runs of `var = var; **note;`
//! assignments drift the same way.
//!
//! Both passes work like `terraform fmt`: detect the structure, split
//! on the whitespace runs the tabs left behind (2+ spaces = column
//! separator), and re-emit with consistent columns. They are pure
//! whitespace rewrites — cell text, comment text, and code are never
//! altered — and both are idempotent.

use std::collections::HashSet;

use once_cell::sync::Lazy;
use regex::Regex;
use sas_lexer::{TokenChannel, TokenType};

use crate::token::TokenStream;

/// Minimum gap between aligned columns, and between code and an
/// end-of-line comment.
const GAP: usize = 2;

/// Realign banner comment blocks: runs of two or more consecutive
/// lines that are each a star border (`****…;`), a dash divider
/// (`** ---- **;`), or a visual comment (`**  …  **;`). Within a block,
/// interior text is split into cells on runs of 2+ spaces, cells are
/// aligned into columns (column widths reset at each divider/border),
/// and every line is padded so the `**;` terminators — and the
/// borders/dividers themselves — share one uniform width.
pub fn align_comment_banners(source: &str) -> String {
    let protected = protected_lines(source);
    let lines = split_lines(source);
    let rows: Vec<Option<BannerRow>> = lines
        .iter()
        .enumerate()
        .map(|(i, (body, _))| {
            if protected.contains(&((i + 1) as u32)) {
                None
            } else {
                classify_banner_line(body)
            }
        })
        .collect();

    let mut out = String::with_capacity(source.len() + 128);
    let mut i = 0;
    while i < lines.len() {
        if rows[i].is_none() {
            out.push_str(lines[i].0);
            out.push_str(lines[i].1);
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && rows[i].is_some() {
            i += 1;
        }
        render_banner_block(&lines[start..i], &rows[start..i], &mut out);
    }
    out
}

/// Align `=` and trailing comments across runs of simple assignment
/// statements (`name = expr;` optionally followed by a `*…;` or
/// `/*…*/` comment). Blank lines do not break a run — InterRAI files
/// separate each assignment with one — but any other line does. Runs
/// of fewer than two assignments are left untouched.
pub fn align_assignments(source: &str) -> String {
    let protected = protected_lines(source);
    let lines = split_lines(source);
    let parsed: Vec<Option<Assign>> = lines
        .iter()
        .enumerate()
        .map(|(i, (body, _))| {
            if protected.contains(&((i + 1) as u32)) {
                None
            } else {
                parse_assign(body)
            }
        })
        .collect();

    let mut out = String::with_capacity(source.len() + 64);
    let mut i = 0;
    while i < lines.len() {
        if parsed[i].is_none() {
            out.push_str(lines[i].0);
            out.push_str(lines[i].1);
            i += 1;
            continue;
        }
        // Extend the run: assignment lines, with blank lines allowed
        // between them. Trailing blanks after the last assignment stay
        // outside the run.
        let start = i;
        let mut members: Vec<usize> = Vec::new();
        let mut last_assign = i;
        let mut j = i;
        while j < lines.len() {
            if parsed[j].is_some() {
                members.push(j);
                last_assign = j;
                j += 1;
            } else if lines[j].0.trim().is_empty() {
                j += 1;
            } else {
                break;
            }
        }

        if members.len() < 2 {
            out.push_str(lines[start].0);
            out.push_str(lines[start].1);
            i = start + 1;
            continue;
        }

        let eq_col = members
            .iter()
            .map(|&m| {
                let a = parsed[m].as_ref().unwrap();
                char_len(&a.indent) + char_len(&a.name)
            })
            .max()
            .unwrap();
        let codes: Vec<String> = members
            .iter()
            .map(|&m| {
                let a = parsed[m].as_ref().unwrap();
                let mut code = String::new();
                code.push_str(&a.indent);
                code.push_str(&a.name);
                pad_to(&mut code, eq_col);
                code.push_str(" = ");
                code.push_str(&a.expr);
                code.push(';');
                code
            })
            .collect();
        let comment_col = codes.iter().map(|c| char_len(c)).max().unwrap() + GAP;

        let mut next_member = 0;
        for idx in start..=last_assign {
            if next_member < members.len() && members[next_member] == idx {
                let a = parsed[idx].as_ref().unwrap();
                let mut line = codes[next_member].clone();
                if let Some(comment) = &a.comment {
                    pad_to(&mut line, comment_col);
                    line.push_str(comment);
                }
                out.push_str(&line);
                next_member += 1;
            } else {
                out.push_str(lines[idx].0);
            }
            out.push_str(lines[idx].1);
        }
        i = last_assign + 1;
    }
    out
}

// ── banner internals ──────────────────────────────────────────────────

#[derive(Debug)]
enum BannerRow {
    /// A full-width star line: `****…****;`
    Border,
    /// A dash divider inside the box: `** ------- **;`
    Divider,
    /// A `**  …  **;` line; `indent` is the leading-space count of the
    /// interior, `cells` the interior split on runs of 2+ spaces.
    Content { indent: usize, cells: Vec<String> },
}

static CELL_GAP: Lazy<Regex> = Lazy::new(|| Regex::new(r"  +").unwrap());

fn classify_banner_line(body: &str) -> Option<BannerRow> {
    let t = body.trim_end();
    if t.len() >= 5 && t.ends_with(';') && t[..t.len() - 1].chars().all(|c| c == '*') {
        return Some(BannerRow::Border);
    }
    if t.len() >= 6 && t.starts_with("**") && t.ends_with("**;") {
        let interior = &t[2..t.len() - 3];
        let trimmed = interior.trim();
        if !trimmed.is_empty() && trimmed.chars().all(|c| c == '-') {
            return Some(BannerRow::Divider);
        }
        let indent = char_len(interior) - char_len(interior.trim_start());
        let cells: Vec<String> = if trimmed.is_empty() {
            Vec::new()
        } else {
            CELL_GAP.split(trimmed).map(str::to_string).collect()
        };
        return Some(BannerRow::Content { indent, cells });
    }
    None
}

fn render_banner_block(lines: &[(&str, &str)], rows: &[Option<BannerRow>], out: &mut String) {
    let content_rows = rows
        .iter()
        .filter(|r| matches!(r, Some(BannerRow::Content { .. })))
        .count();
    // A lone `**  NOTE  **;` line is not a banner box — leave it alone.
    if rows.len() < 2 || content_rows == 0 {
        for (body, term) in lines {
            out.push_str(body);
            out.push_str(term);
        }
        return;
    }

    // Column widths per group; a border or divider starts a new group
    // (e.g. the header fields above the `** ---- **;` line align
    // independently of the variable table below it). A cell only
    // widens its column when the row has more cells after it, so a
    // long trailing note doesn't stretch the column it starts in.
    let mut group_of = vec![0usize; rows.len()];
    let mut widths: Vec<Vec<usize>> = vec![Vec::new()];
    for (idx, row) in rows.iter().enumerate() {
        match row.as_ref().unwrap() {
            BannerRow::Border | BannerRow::Divider => {
                widths.push(Vec::new());
                group_of[idx] = widths.len() - 1;
            }
            BannerRow::Content { indent, cells } => {
                group_of[idx] = widths.len() - 1;
                let w = widths.last_mut().unwrap();
                if cells.len() >= 2 {
                    for k in 0..cells.len() - 1 {
                        let cw = if k == 0 {
                            indent + char_len(&cells[0])
                        } else {
                            char_len(&cells[k])
                        };
                        if w.len() <= k {
                            w.resize(k + 1, 0);
                        }
                        w[k] = w[k].max(cw);
                    }
                }
            }
        }
    }

    let rendered: Vec<Option<String>> = rows
        .iter()
        .enumerate()
        .map(|(idx, row)| match row.as_ref().unwrap() {
            BannerRow::Content { indent, cells } => {
                if cells.is_empty() {
                    return Some(String::new());
                }
                let w = &widths[group_of[idx]];
                let mut s = String::new();
                for _ in 0..*indent {
                    s.push(' ');
                }
                s.push_str(&cells[0]);
                let mut col = 0usize;
                for (k, cell) in cells.iter().enumerate().skip(1) {
                    col += w[k - 1] + GAP;
                    pad_to(&mut s, col);
                    s.push_str(cell);
                }
                Some(s)
            }
            _ => None,
        })
        .collect();

    // Uniform total width: widest content line plus the minimum gap
    // and the `**;` terminator; borders and dividers stretch to match.
    let width = rendered
        .iter()
        .flatten()
        .map(|r| 2 + char_len(r) + GAP + 3)
        .max()
        .unwrap()
        .max(12);

    for (idx, row) in rows.iter().enumerate() {
        match row.as_ref().unwrap() {
            BannerRow::Border => {
                out.push_str(&"*".repeat(width - 1));
                out.push(';');
            }
            BannerRow::Divider => {
                out.push_str("** ");
                out.push_str(&"-".repeat(width - 7));
                out.push_str(" **;");
            }
            BannerRow::Content { .. } => {
                let r = rendered[idx].as_ref().unwrap();
                out.push_str("**");
                out.push_str(r);
                for _ in 0..(width - 5 - char_len(r)) {
                    out.push(' ');
                }
                out.push_str("**;");
            }
        }
        out.push_str(lines[idx].1);
    }
}

// ── assignment internals ──────────────────────────────────────────────

#[derive(Debug)]
struct Assign {
    indent: String,
    name: String,
    expr: String,
    comment: Option<String>,
}

/// A whole-line simple assignment: `name = expr;` plus an optional
/// trailing comment. The expression may not contain `;` or quotes —
/// anything fancier is left for a human.
static ASSIGN_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"^([ \t]*)([A-Za-z_][A-Za-z0-9_]*)[ \t]*=[ \t]*([^;'"]+?)[ \t]*;[ \t]*(\*.*;|/\*.*\*/)?[ \t]*$"#)
        .unwrap()
});

fn parse_assign(body: &str) -> Option<Assign> {
    let caps = ASSIGN_RE.captures(body)?;
    Some(Assign {
        indent: caps[1].to_string(),
        name: caps[2].to_string(),
        expr: caps[3].to_string(),
        comment: caps.get(4).map(|m| m.as_str().to_string()),
    })
}

// ── shared helpers ────────────────────────────────────────────────────

/// Lines whose bytes are the interior of a multi-line comment, string
/// literal, or a datalines block. Alignment passes must never rewrite
/// these: the text is prose or data, not statements, and whitespace in
/// datalines is significant.
fn protected_lines(source: &str) -> HashSet<u32> {
    let stream = TokenStream::tokenize(source);
    let mut set = HashSet::new();
    for t in &stream.all {
        if t.channel == TokenChannel::HIDDEN {
            continue;
        }
        let is_protected_kind = t.channel == TokenChannel::COMMENT
            || matches!(
                t.token_type,
                TokenType::StringLiteral | TokenType::DatalinesData
            );
        if !is_protected_kind {
            continue;
        }
        if t.token_type == TokenType::DatalinesData {
            for l in t.start_line..=t.end_line {
                set.insert(l);
            }
        } else if t.end_line > t.start_line {
            for l in (t.start_line + 1)..=t.end_line {
                set.insert(l);
            }
        }
    }
    set
}

/// Split into `(body, terminator)` pairs, preserving `\n` vs `\r\n`.
fn split_lines(source: &str) -> Vec<(&str, &str)> {
    source
        .split_inclusive('\n')
        .map(|raw| {
            if let Some(rest) = raw.strip_suffix("\r\n") {
                (rest, "\r\n")
            } else if let Some(rest) = raw.strip_suffix('\n') {
                (rest, "\n")
            } else {
                (raw, "")
            }
        })
        .collect()
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

fn pad_to(s: &mut String, col: usize) {
    let mut len = char_len(s);
    while len < col {
        s.push(' ');
        len += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_aligns_terminators_and_columns() {
        let src = "\
****************;\n\
**  PROGRAM:      FOO.txt        **;\n\
**  BY:   ALICE           **;\n\
** ------ **;\n\
**  Age at assessment      sAGE    18-120  **;\n\
**  Gender   iA2bis     1-3       **;\n\
****************;\n";
        let out = align_comment_banners(src);
        let lines: Vec<&str> = out.lines().collect();
        // All lines share one width.
        let w = lines[0].len();
        assert!(lines.iter().all(|l| l.len() == w), "got:\n{out}");
        // Header labels align their value column.
        assert!(lines[1].contains("PROGRAM:  FOO.txt"), "got:\n{out}");
        assert!(lines[2].contains("BY:       ALICE"), "got:\n{out}");
        // Table columns align across rows.
        let c1 = lines[4].find("sAGE").unwrap();
        let c2 = lines[5].find("iA2bis").unwrap();
        assert_eq!(c1, c2, "got:\n{out}");
        let v1 = lines[4].find("18-120").unwrap();
        let v2 = lines[5].find("1-3").unwrap();
        assert_eq!(v1, v2, "got:\n{out}");
    }

    #[test]
    fn banner_pass_is_idempotent() {
        let src = "\
****************;\n\
**  PROGRAM:      FOO.txt        **;\n\
**  Age at assessment      sAGE    18-120  **;\n\
**       note, only when sCPS is 0 or 1    **;\n\
**            **;\n\
** ------ **;\n";
        let once = align_comment_banners(src);
        assert_eq!(align_comment_banners(&once), once);
    }

    #[test]
    fn lone_banner_line_is_untouched() {
        let src = "data one;\n**  START OF SAS CODE  **;\nrun;\n";
        assert_eq!(align_comment_banners(src), src);
    }

    #[test]
    fn assignments_align_equals_and_comments() {
        let src = "\
sAGE    =    sAGE;     **Age at assessment;\n\
\n\
iA2bis    =    iA2bis;     **Gender;\n\
\n\
iI1dter    =    iI1dter;**Dementia;\n";
        let out = align_assignments(src);
        let expected = "\
sAGE    = sAGE;     **Age at assessment;\n\
\n\
iA2bis  = iA2bis;   **Gender;\n\
\n\
iI1dter = iI1dter;  **Dementia;\n";
        assert_eq!(out, expected);
    }

    #[test]
    fn assignment_pass_is_idempotent() {
        let src = "a = 1;\nlonger_name=2; *note;\n";
        let once = align_assignments(src);
        assert_eq!(align_assignments(&once), once);
    }

    #[test]
    fn single_assignment_is_untouched() {
        let src = "/*Age in 5-year increments*/\nAGE  =  sAGE/5;\n\nif a then b;\n";
        assert_eq!(align_assignments(src), src);
    }

    #[test]
    fn conditional_lines_do_not_join_groups() {
        let src = "if iA4=1        then marital1=1; else marital1=0; *never married;\n\
                   if iA4=2        then marital2=1; else marital2=0; *married;\n";
        assert_eq!(align_assignments(src), src);
    }

    #[test]
    fn datalines_are_never_rewritten() {
        let src = "data x;\ninput a b;\ndatalines;\na = 1;\nb  =  2;\n;\nrun;\n";
        assert_eq!(align_assignments(src), src);
    }

    #[test]
    fn block_comment_interiors_are_never_rewritten() {
        let src = "/* first\nx  =  1;\ny  =  2;\n*/\n";
        assert_eq!(align_assignments(src), src);
        assert_eq!(align_comment_banners(src), src);
    }
}
