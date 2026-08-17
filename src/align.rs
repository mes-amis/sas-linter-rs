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
///
/// A line holding nothing but several complete `**…**;` comments is
/// first split into one comment per line.
///
/// A block whose lines already share one uniform width is left
/// byte-identical — it was laid out deliberately, and there is nothing
/// to fix.
pub fn align_comment_banners(source: &str) -> String {
    let source = split_stacked_banner_comments(source);
    let source = source.as_str();
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
    /// A dash or star divider inside the box (`** ------- **;` or
    /// `** ******* **;`); `fill` is whichever character was used, so
    /// re-rendering doesn't change one into the other.
    Divider { fill: char },
    /// A `**  …  **;` line; `indent` is the leading-space count of the
    /// interior, `cells` the interior split on runs of 2+ spaces.
    Content { indent: usize, cells: Vec<String> },
}

static CELL_GAP: Lazy<Regex> = Lazy::new(|| Regex::new(r"  +").unwrap());

static BANNER_COMMENT: Lazy<Regex> = Lazy::new(|| Regex::new(r"\*\*.*?\*\*;").unwrap());

/// Split banner interior text into cells on runs of 2+ spaces, except a
/// run immediately preceded by a period. "Two spaces after a full stop"
/// is a common prose typing convention, and source headers routinely
/// wrap a long description across several banner lines this way;
/// treating every such gap as a column boundary misreads a wrapped
/// sentence as a two-cell label/value row. Its wide first "cell" then
/// dominates the column-width computation for the whole banner group,
/// dragging every genuine label:value row — and, via the
/// uniform-total-width pass, the entire box — out to match it. Genuine
/// label rows split on a colon (`PROGRAM:  FOO.txt`), never a period, so
/// this is safe to skip without touching real column boundaries.
fn split_cells(trimmed: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut cell_start = 0usize;
    for m in CELL_GAP.find_iter(trimmed) {
        if trimmed[..m.start()].ends_with('.') {
            continue;
        }
        cells.push(trimmed[cell_start..m.start()].to_string());
        cell_start = m.end();
    }
    cells.push(trimmed[cell_start..].to_string());
    cells
}

/// Split a line that consists solely of two or more complete `**…**;`
/// comments (`** A **;   ** B **;`) into one comment per line, each
/// keeping the original leading indent. Lines with any other content
/// mixed in are left alone.
fn split_stacked_banner_comments(source: &str) -> String {
    let protected = protected_lines(source);
    let lines = split_lines(source);
    let mut out = String::with_capacity(source.len() + 64);
    for (i, (body, term)) in lines.iter().enumerate() {
        if protected.contains(&((i + 1) as u32)) {
            out.push_str(body);
            out.push_str(term);
            continue;
        }
        let indent_len = body.len() - body.trim_start_matches([' ', '\t']).len();
        let (indent, rest) = body.split_at(indent_len);
        let t = rest.trim_end();
        let mut segments: Vec<&str> = Vec::new();
        let mut pos = 0;
        let mut clean = true;
        for m in BANNER_COMMENT.find_iter(t) {
            if !t[pos..m.start()].trim().is_empty() {
                clean = false;
                break;
            }
            segments.push(m.as_str());
            pos = m.end();
        }
        clean = clean && t[pos..].trim().is_empty();
        if clean && segments.len() >= 2 {
            let newline = if term.is_empty() { "\n" } else { *term };
            for (k, seg) in segments.iter().enumerate() {
                out.push_str(indent);
                out.push_str(seg);
                out.push_str(if k + 1 < segments.len() {
                    newline
                } else {
                    term
                });
            }
        } else {
            out.push_str(body);
            out.push_str(term);
        }
    }
    out
}

fn classify_banner_line(body: &str) -> Option<BannerRow> {
    let t = body.trim_end();
    if t.len() >= 5 && t.ends_with(';') && t[..t.len() - 1].chars().all(|c| c == '*') {
        return Some(BannerRow::Border);
    }
    if t.len() >= 6 && t.starts_with("**") && t.ends_with("**;") {
        let interior = &t[2..t.len() - 3];
        let trimmed = interior.trim();
        // A divider wrapped in the same `**  ...  **;` shell as an
        // ordinary content line (`**********...  **;`) rather than a
        // bare `****...;` border. Also recognized on '*': some sources
        // use a run of asterisks as a stronger in-box separator than
        // the dash divider. Either way it's decorative, not data, and
        // must still reset the group like a bare border does — missed,
        // it reads as one huge content cell that merges the sections on
        // either side into a single alignment group.
        if let Some(fill) = trimmed.chars().next() {
            if matches!(fill, '-' | '*') && trimmed.chars().all(|c| c == fill) {
                return Some(BannerRow::Divider { fill });
            }
        }
        let indent = char_len(interior) - char_len(interior.trim_start());
        let cells: Vec<String> = if trimmed.is_empty() {
            Vec::new()
        } else {
            split_cells(trimmed)
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

    // A block whose lines already share one uniform width is a
    // deliberately laid-out box: the `**;` terminators align and every
    // line was hand-padded to hit the same edge. There is nothing to
    // repair, and re-deriving cell columns risks misreading intentional
    // layout — a table header with a different cell count than its data
    // rows, or a full-width cell whose value sits one space away — and
    // rewriting the whole banner around the misread. Leave it
    // byte-identical.
    let mut line_widths = lines.iter().map(|(body, _)| char_len(body.trim_end()));
    let first_width = line_widths.next().unwrap();
    if line_widths.all(|w| w == first_width) {
        for (body, term) in lines {
            out.push_str(body);
            out.push_str(term);
        }
        return;
    }

    // Assign rows to groups; a border or divider starts a new group
    // (e.g. the header fields above the `** ---- **;` line align
    // independently of the variable table below it).
    let mut group_of = vec![0usize; rows.len()];
    let mut group_count = 1usize;
    for (idx, row) in rows.iter().enumerate() {
        if matches!(
            row.as_ref().unwrap(),
            BannerRow::Border | BannerRow::Divider { .. }
        ) {
            group_count += 1;
        }
        group_of[idx] = group_count - 1;
    }

    // Pull wrapped continuations of an already-valued field out of their
    // enclosing group. A multi-cell row indented more than one level
    // past the content row directly above it, when that row already
    // carries a value (2+ cells), is text wrapping onto a second line —
    // e.g. `SOURCE:  A short reference` followed by
    // `SEE ALSO THE FOLLOWING RELATED DOCUMENT:  NONE`. Left in the
    // parent group, that continuation's own wide "label" cell would
    // dominate the column-width computation and drag every sibling
    // label's value column out to match it. One level deeper is still
    // the existing stray-space case above and snaps to the group's
    // modal indent as before; each continuation instead gets its own
    // singleton group, rendered against a column width of just itself.
    for idx in 1..rows.len() {
        let is_continuation = match (rows[idx].as_ref().unwrap(), rows[idx - 1].as_ref().unwrap()) {
            (
                BannerRow::Content { indent, cells },
                BannerRow::Content {
                    indent: prev_indent,
                    cells: prev_cells,
                },
            ) => cells.len() >= 2 && prev_cells.len() >= 2 && *indent > prev_indent + 1,
            _ => false,
        };
        if is_continuation {
            group_of[idx] = group_count;
            group_count += 1;
        }
    }

    // Modal interior indent per group, over multi-cell rows. A stray
    // extra space in the original tab-aligned text pushes a row one
    // column right of its siblings; a multi-cell row sitting exactly
    // one space deeper than the group's most common indent snaps back
    // onto it. Ties prefer the shallower indent.
    let mut indent_counts: Vec<std::collections::HashMap<usize, usize>> =
        vec![std::collections::HashMap::new(); group_count];
    for (idx, row) in rows.iter().enumerate() {
        if let BannerRow::Content { indent, cells } = row.as_ref().unwrap() {
            if cells.len() >= 2 {
                *indent_counts[group_of[idx]].entry(*indent).or_insert(0) += 1;
            }
        }
    }
    let modal_indent: Vec<Option<usize>> = indent_counts
        .iter()
        .map(|counts| {
            counts
                .iter()
                .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
                .map(|(indent, _)| *indent)
        })
        .collect();
    let effective_indent = |idx: usize, indent: usize, cells: &[String]| -> usize {
        if cells.len() >= 2 {
            if let Some(m) = modal_indent[group_of[idx]] {
                if indent == m + 1 {
                    return m;
                }
            }
        }
        indent
    };

    // Column widths per group. A cell only widens its column when the
    // row has more cells after it, so a long trailing note doesn't
    // stretch the column it starts in.
    let mut widths: Vec<Vec<usize>> = vec![Vec::new(); group_count];
    for (idx, row) in rows.iter().enumerate() {
        if let BannerRow::Content { indent, cells } = row.as_ref().unwrap() {
            let w = &mut widths[group_of[idx]];
            if cells.len() >= 2 {
                let indent = effective_indent(idx, *indent, cells);
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
                for _ in 0..effective_indent(idx, *indent, cells) {
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
            BannerRow::Divider { fill } => {
                out.push_str("** ");
                out.push_str(&fill.to_string().repeat(width - 7));
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
    fn already_rectangular_banner_is_left_alone() {
        // Every line shares one width: the box was laid out by hand and
        // must stay byte-identical. The rows are shapes the column
        // re-derivation used to mangle — a table header whose cell count
        // differs from its data rows, a row whose long first cell sits a
        // single space from its value, and a label group whose values
        // were deliberately not column-aligned.
        let width = 66;
        let border = format!("{};\n", "*".repeat(width - 1));
        let divider = format!("** {} **;\n", "-".repeat(width - 7));
        let content = |s: &str| {
            let mut l = format!("**{s}");
            while l.chars().count() < width - 3 {
                l.push(' ');
            }
            l.push_str("**;\n");
            l
        };
        let src = format!(
            "{border}{}{}{}{}{}{}{}{border}",
            content("  ASSUMPTIONS:  NONE"),
            content("  ALGORITHM:        Output follows a decision tree"),
            divider,
            content("       DESCRIPTION                  NAME     VALUES   NOTES"),
            content("   A description too long for its column xQ7a  0,1"),
            content("   Short description                xQ7b     0-5"),
            content("   Another description              xQ8a     0,1,8"),
        );
        assert_eq!(align_comment_banners(&src), src);
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
    fn off_by_one_indent_snaps_to_group_mode() {
        let src = "\
**  Age at assessment      sAGE    18-120  **;\n\
**   Cognitive Performance Scale   sCPS   0-1   **;\n\
**  Gender   iA2bis     1-3       **;\n";
        let out = align_comment_banners(src);
        let lines: Vec<&str> = out.lines().collect();
        // The row with one extra leading space joins its siblings' column.
        assert!(lines[1].starts_with("**  Cognitive"), "got:\n{out}");
        let c1 = lines[0].find("sAGE").unwrap();
        let c2 = lines[1].find("sCPS").unwrap();
        assert_eq!(c1, c2, "got:\n{out}");
    }

    #[test]
    fn double_space_after_period_is_not_a_column_boundary() {
        // "Two spaces after a full stop" is ordinary prose typing, not a
        // table gap. A wrapped sentence like this must not be read as a
        // two-cell label/value row.
        let src = "\
**  PROGRAM:      FOO.txt        **;\n\
**  DESCRIPTION:  A SHORT SUMMARY OF WHAT THIS PROGRAM DOES **;\n\
**                FOR TWO REASONS.  THE SECOND REASON IS LONG **;\n";
        let out = align_comment_banners(src);
        let lines: Vec<&str> = out.lines().collect();
        // The wrapped line keeps its original single-cell spacing rather
        // than being padded out to a column position.
        assert!(
            lines[2].contains("FOR TWO REASONS.  THE SECOND REASON IS LONG"),
            "got:\n{out}"
        );
        // Unaffected: the genuine label rows still align on their colon.
        assert!(lines[0].contains("PROGRAM:      FOO.txt"), "got:\n{out}");
    }

    #[test]
    fn deep_continuation_of_a_valued_label_does_not_widen_the_group() {
        // A label whose value wraps onto an indented second line that
        // itself looks like `LABEL:  VALUE` (e.g. a long sub-reference)
        // must not drag the shallow label rows' value column out to
        // match its own, much deeper, indent.
        let src = "\
**  PROGRAM:      FOO.txt        **;\n\
**  BY:           ALICE          **;\n\
**  SOURCE:       A SHORT REFERENCE **;\n\
**                SEE ALSO THE FOLLOWING RELATED DOCUMENT:  NONE **;\n\
**  APPLIES TO:   EVERYONE       **;\n";
        let out = align_comment_banners(src);
        let lines: Vec<&str> = out.lines().collect();
        // The shallow label rows keep a tight, shared column — not one
        // stretched out to fit the deep sub-reference line's own label.
        let c1 = lines[0].find("FOO.txt").unwrap();
        let c2 = lines[1].find("ALICE").unwrap();
        let c3 = lines[4].find("EVERYONE").unwrap();
        assert_eq!(c1, c2, "got:\n{out}");
        assert_eq!(c1, c3, "got:\n{out}");
        // The deep continuation still renders sensibly against its own
        // column, independent of the shallow rows around it.
        assert!(
            lines[3].contains("SEE ALSO THE FOLLOWING RELATED DOCUMENT:  NONE"),
            "got:\n{out}"
        );
        // The whole box no longer balloons out to accommodate the one
        // deep line: total width stays close to the longest shallow row.
        assert!(lines[0].len() < 70, "got:\n{out}");
    }

    #[test]
    fn continuation_pass_is_idempotent() {
        let src = "\
**  PROGRAM:      FOO.txt        **;\n\
**  SOURCE:       A SHORT REFERENCE **;\n\
**                SEE ALSO THE FOLLOWING RELATED DOCUMENT:  NONE **;\n";
        let once = align_comment_banners(src);
        assert_eq!(align_comment_banners(&once), once);
    }

    #[test]
    fn star_run_wrapped_in_content_shell_is_a_divider_not_content() {
        // `**********...  **;` is a divider dressed in the same shell as
        // an ordinary content line, not a bare `****...;` border. Missed,
        // it reads as one giant content cell and merges the section
        // above it with the section below into a single alignment
        // group — the two sections must stay independently aligned.
        let src = "\
**  SHORT LABEL:   A short value **;\n\
**  A MUCH LONGER LABEL HERE:  its value **;\n\
**********************************  **;\n\
** UNRELATED PARAGRAPH TEXT AND MORE **;\n\
** SECOND. LINE OF THAT PARAGRAPH **;\n";
        let out = align_comment_banners(src);
        let lines: Vec<&str> = out.lines().collect();
        // The divider is still recognized as one (uniform box width,
        // stretched border/divider fill) ...
        assert!(lines[2].trim_start_matches("**").trim().starts_with('*'));
        // ... and the paragraph below it never had to widen to match
        // the long label above it: it keeps its own short column.
        assert!(lines[3].len() < 60, "got:\n{out}");
        // The star fill is preserved rather than rewritten to dashes.
        assert!(
            lines[2].contains('*') && !lines[2].contains('-'),
            "got:\n{out}"
        );
    }

    #[test]
    fn dash_and_star_dividers_keep_their_own_fill_character() {
        let src = "\
**  PROGRAM:  FOO.txt  **;\n\
** ----- **;\n\
** ***** **;\n";
        let out = align_comment_banners(src);
        let lines: Vec<&str> = out.lines().collect();
        // Strip the `** ` / ` **;` wrapper before checking fill — both
        // lines legitimately contain '*' there regardless of fill.
        let fill_of = |l: &str| {
            l.trim_start_matches("** ")
                .trim_end_matches(" **;")
                .to_string()
        };
        assert!(fill_of(lines[1]).chars().all(|c| c == '-'), "got:\n{out}");
        assert!(fill_of(lines[2]).chars().all(|c| c == '*'), "got:\n{out}");
    }

    #[test]
    fn stacked_comments_split_one_per_line() {
        let src =
            "data one;\n** VARIABLE ASSIGNMENTS **;     ** PUT YOUR VARIABLES HERE **;\nx = 1;\n";
        let out = align_comment_banners(src);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 4, "got:\n{out}");
        assert!(
            lines[1].starts_with("** VARIABLE ASSIGNMENTS"),
            "got:\n{out}"
        );
        assert!(
            lines[2].starts_with("** PUT YOUR VARIABLES HERE"),
            "got:\n{out}"
        );
        // The split pair aligns as its own block: same width.
        assert_eq!(lines[1].len(), lines[2].len(), "got:\n{out}");
        // Idempotent.
        assert_eq!(align_comment_banners(&out), out);
    }

    #[test]
    fn single_comment_with_interior_semicolon_is_not_split() {
        let src = "**  REVISION DATES:   01/01/00; 02/02/01 **;\n";
        assert_eq!(align_comment_banners(src), src);
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
