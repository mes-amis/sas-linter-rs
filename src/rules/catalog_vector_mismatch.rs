//! Cross-check each acceptable-values catalog row against its own vector
//! column.
//!
//! A catalog row states its values twice — compactly (`Acceptable Values`:
//! `1-3,8`) and spelled out (`Vector`: `1,2,3,8`). When the two drift
//! apart (a vector generated as every whole number from min to max,
//! `1,2,3,4,5,6,7,8`, silently admits the gap `4-7`) the linter and the
//! data disagree about what a variable can hold. This reports every row
//! whose two columns name different values, and rows where only one
//! column is a value list — usually a description with an unquoted
//! delimiter that shifted the columns over.
//!
//! The findings are about the catalog, not a source, so they come from
//! `check_inputs` — once per run, located at the CSV row.

use std::path::Path;

use once_cell::sync::Lazy;
use regex::Regex;

use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};
use crate::value_specs::SpecSource;

use super::RuleMeta;

pub struct CatalogVectorMismatch {
    source: SpecSource,
}

const ID: &str = "catalog_vector_mismatch";
const DESCRIPTION: &str = "Acceptable-values catalog row whose values and \
                            vector columns name different values.";

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: false,
        default_factory: || {
            Box::new(CatalogVectorMismatch {
                source: SpecSource::default(),
            })
        },
        config_factory: |opts| {
            Ok(Box::new(CatalogVectorMismatch {
                source: SpecSource::from_opts(opts)?,
            }))
        },
    }
}

impl Rule for CatalogVectorMismatch {
    fn id(&self) -> &'static str {
        ID
    }
    fn description(&self) -> &'static str {
        DESCRIPTION
    }

    fn check(&self, _ctx: &CheckContext) -> Vec<Finding> {
        Vec::new()
    }

    fn check_inputs(&self) -> Vec<Finding> {
        self.source
            .csv_paths
            .iter()
            .flat_map(|path| check_file(path, &self.source))
            .collect()
    }
}

/// A range at least this wide counts as too long to spell out, so a
/// vector of just its two endpoints (`0,9999999999999` for
/// `0-9999999999999`) agrees with it.
const SPELLED_OUT_LIMIT: i64 = 1000;

/// Values shown per column before the text is cut short.
const SHOWN_CHARS: usize = 40;

/// Check one catalog file. Like `SpecSource::load`, a missing or
/// unreadable file, or one without the name, values and vector columns,
/// is skipped silently — a catalog with no vector column has nothing to
/// cross-check.
fn check_file(path: &Path, source: &SpecSource) -> Vec<Finding> {
    let Ok(mut rdr) = csv::ReaderBuilder::new()
        .delimiter(source.delimiter)
        .has_headers(true)
        .flexible(true)
        .from_path(path)
    else {
        return Vec::new();
    };
    let Ok(headers) = rdr.headers().cloned() else {
        return Vec::new();
    };
    // A UTF-8 BOM sticks to the first header.
    let column = |name: &str| {
        headers
            .iter()
            .position(|h| h.trim_start_matches('\u{feff}') == name)
    };
    let (Some(name_col), Some(values_col), Some(vector_col)) = (
        column(&source.name_column),
        column(&source.values_column),
        column(&source.vector_column),
    ) else {
        return Vec::new();
    };

    let mut findings = Vec::new();
    for record in rdr.records().flatten() {
        let line = record.position().map_or(0, |p| p.line());
        let name = record.get(name_col).unwrap_or("").trim();
        let values = record.get(values_col).unwrap_or("").trim();
        let vector = record.get(vector_col).unwrap_or("").trim();
        if let Some(message) = compare(&source.values_column, values, &source.vector_column, vector)
        {
            findings.push(Finding {
                path: path.display().to_string(),
                line: u32::try_from(line).unwrap_or(u32::MAX),
                column: 1,
                rule: ID,
                message: format!("{name}: {message}"),
                severity: Severity::Warning,
            });
        }
    }
    findings
}

/// What's wrong between a row's values text and its vector text, if
/// anything.
fn compare(values_label: &str, values: &str, vector_label: &str, vector: &str) -> Option<String> {
    let shown_values = format!("{values_label} {}", show(values));
    let shown_vector = format!("{vector_label} {}", show(vector));
    match (parse_list(values), parse_list(vector)) {
        (None, None) => None,
        (Some(_), None) | (None, Some(_)) if values.is_empty() || vector.is_empty() => {
            let (empty, other, text) = if values.is_empty() {
                (values_label, vector_label, vector)
            } else {
                (vector_label, values_label, values)
            };
            Some(format!("{empty} is empty but {other} is {}", show(text)))
        }
        (Some(_), None) => Some(format!(
            "{shown_values} is a value list but {shown_vector} is not (shifted column?)"
        )),
        (None, Some(_)) => Some(format!(
            "{shown_vector} is a value list but {shown_values} is not (shifted column?)"
        )),
        (Some(a), Some(b)) => {
            if a == b || endpoints_of_long_range(&a, &b) || endpoints_of_long_range(&b, &a) {
                return None;
            }
            let mut parts = Vec::new();
            let extra = difference(&b, &a);
            if !extra.is_empty() {
                parts.push(format!("{vector_label} adds {}", render(&extra)));
            }
            let missing = difference(&a, &b);
            if !missing.is_empty() {
                parts.push(format!("{vector_label} omits {}", render(&missing)));
            }
            Some(format!(
                "{shown_values} and {shown_vector} disagree: {}",
                parts.join("; ")
            ))
        }
    }
}

fn show(text: &str) -> String {
    if text.chars().count() <= SHOWN_CHARS {
        format!("`{text}`")
    } else {
        let cut: String = text.chars().take(SHOWN_CHARS).collect();
        format!("`{cut}…`")
    }
}

/// Inclusive `(lo, hi)` intervals, sorted, with overlapping and adjacent
/// ones merged — so equal value sets compare equal.
type Intervals = Vec<(i64, i64)>;

static RE_INT: Lazy<Regex> = Lazy::new(|| Regex::new(r"^-?\d+$").unwrap());
static RE_SPAN: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(-?\d+)\s*-\s*(-?\d+)$").unwrap());

/// Read a comma-separated list of whole numbers and `lo-hi` ranges
/// (`1-3,8`, `0,1,2`, `1-5 (8,9)`). Anything else — text, `yyyymmdd`
/// dates, decimal ranges — is `None`.
fn parse_list(text: &str) -> Option<Intervals> {
    if text.is_empty() {
        return None;
    }
    let text = text.replace('(', ",").replace(')', "");
    let mut out = Vec::new();
    for item in text.split(',').map(str::trim) {
        if item.is_empty() {
            continue;
        }
        if RE_INT.is_match(item) {
            let n: i64 = item.parse().ok()?;
            out.push((n, n));
        } else {
            let c = RE_SPAN.captures(item)?;
            let lo: i64 = c[1].parse().ok()?;
            let hi: i64 = c[2].parse().ok()?;
            if hi < lo {
                return None;
            }
            out.push((lo, hi));
        }
    }
    if out.is_empty() {
        return None;
    }
    out.sort_unstable();
    let mut merged: Intervals = Vec::with_capacity(out.len());
    for (lo, hi) in out {
        match merged.last_mut() {
            Some(last) if lo <= last.1.saturating_add(1) => last.1 = last.1.max(hi),
            _ => merged.push((lo, hi)),
        }
    }
    Some(merged)
}

/// `range` is one range too wide to spell out and `list` names just its
/// two ends.
fn endpoints_of_long_range(range: &Intervals, list: &Intervals) -> bool {
    match range[..] {
        [(lo, hi)] if hi.saturating_sub(lo) >= SPELLED_OUT_LIMIT => {
            list[..] == [(lo, lo), (hi, hi)]
        }
        _ => false,
    }
}

/// The values in `a` that `b` lacks.
fn difference(a: &Intervals, b: &Intervals) -> Intervals {
    let mut out = Vec::new();
    for &(lo, hi) in a {
        let mut start = lo;
        for &(blo, bhi) in b {
            if bhi < start || blo > hi {
                continue;
            }
            if blo > start {
                out.push((start, blo - 1));
            }
            start = bhi.saturating_add(1);
            if start > hi {
                break;
            }
        }
        if start <= hi {
            out.push((start, hi));
        }
    }
    out
}

fn render(set: &Intervals) -> String {
    set.iter()
        .map(|&(lo, hi)| match hi - lo {
            0 => lo.to_string(),
            1 => format!("{lo},{hi}"),
            _ => format!("{lo}-{hi}"),
        })
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmp(values: &str, vector: &str) -> Option<String> {
        compare("Acceptable Values", values, "Vector", vector)
    }

    #[test]
    fn agreeing_spellings_are_silent() {
        assert_eq!(cmp("0-2", "0,1,2"), None);
        assert_eq!(cmp("1-3,8", "1,2,3,8"), None);
        assert_eq!(cmp("1-5 (8,9)", "1,2,3,4,5,8,9"), None);
        assert_eq!(cmp("0-9999999999999", "0,9999999999999"), None);
        assert_eq!(cmp("8 Characters: yyyymmdd", "8 Characters"), None);
        assert_eq!(cmp("0.0-99.9", "0.0-99.9"), None);
        assert_eq!(cmp("", ""), None);
    }

    #[test]
    fn gap_filled_vector_is_reported() {
        assert_eq!(
            cmp("1-3,8", "1,2,3,4,5,6,7,8").as_deref(),
            Some(
                "Acceptable Values `1-3,8` and Vector `1,2,3,4,5,6,7,8` disagree: \
                 Vector adds 4-7"
            )
        );
    }

    #[test]
    fn short_vector_is_reported() {
        assert_eq!(
            cmp("0-3", "0,1,3").as_deref(),
            Some("Acceptable Values `0-3` and Vector `0,1,3` disagree: Vector omits 2")
        );
        assert!(cmp("0-5", "0,5").is_some(), "short ranges are spelled out");
    }

    #[test]
    fn shifted_and_empty_columns_are_reported() {
        assert_eq!(
            cmp(" Early onset disorder", "0,1").as_deref(),
            Some(
                "Vector `0,1` is a value list but Acceptable Values \
                 ` Early onset disorder` is not (shifted column?)"
            )
        );
        assert_eq!(
            cmp("0-4", "").as_deref(),
            Some("Vector is empty but Acceptable Values is `0-4`")
        );
    }

    #[test]
    fn difference_splits_intervals() {
        assert_eq!(
            difference(&vec![(0, 10)], &vec![(2, 3), (6, 6)]),
            vec![(0, 1), (4, 5), (7, 10)]
        );
        assert_eq!(render(&vec![(0, 1), (4, 5), (7, 10)]), "0,1,4,5,7-10");
    }
}
