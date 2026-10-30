//! Variable → acceptable-values catalog, loaded from one or more CSV files.
//!
//! Shared by the `variable_value_out_of_known_range` rule (flag literals
//! outside the documented values) and `sas-lint synthesize` (seed each
//! input's value set). Both read the same `csv_paths` / `name_column` /
//! `values_column` / `delimiter` / `name_match` options from the rule's
//! config entry. `sas-lint check-catalog` (`src/catalog_check.rs`) reads
//! them too, plus `vector_column`.

use std::collections::HashMap;
use std::path::PathBuf;

use once_cell::sync::Lazy;
use regex::Regex;

use crate::config::{opt_seq_str, opt_str};

pub const DEFAULT_NAME_COLUMN: &str = "Variable";
pub const DEFAULT_VALUES_COLUMN: &str = "Acceptable Values";
pub const DEFAULT_VECTOR_COLUMN: &str = "Vector";

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NameMatch {
    CaseInsensitive,
    Exact,
}

#[derive(Debug, Clone)]
pub enum SpecKind {
    Range(i64, i64),
    Set(Vec<i64>),
    /// `lo-hi` with a decimal point on either end (`0.0-99.9`): any real
    /// value in the closed interval.
    Decimal(f64, f64),
    /// `8 Characters: yyyymmdd` — a date held as eight characters.
    Date,
}

#[derive(Debug, Clone)]
pub struct Spec {
    pub kind: SpecKind,
}

/// Where to find the catalog and how to read it.
#[derive(Debug, Clone)]
pub struct SpecSource {
    pub csv_paths: Vec<PathBuf>,
    pub name_column: String,
    pub values_column: String,
    /// The column that spells the acceptable values out one by one; read
    /// only by `sas-lint check-catalog`.
    pub vector_column: String,
    pub name_match: NameMatch,
    pub delimiter: u8,
}

impl Default for SpecSource {
    fn default() -> Self {
        SpecSource {
            csv_paths: Vec::new(),
            name_column: DEFAULT_NAME_COLUMN.into(),
            values_column: DEFAULT_VALUES_COLUMN.into(),
            vector_column: DEFAULT_VECTOR_COLUMN.into(),
            name_match: NameMatch::CaseInsensitive,
            delimiter: b',',
        }
    }
}

impl SpecSource {
    /// Build from a rule's YAML options map.
    pub fn from_opts(opts: &serde_yaml::Value) -> anyhow::Result<Self> {
        let name_match = match opt_str(opts, "name_match").unwrap_or("case_insensitive") {
            "exact" => NameMatch::Exact,
            "case_insensitive" => NameMatch::CaseInsensitive,
            other => anyhow::bail!(
                "name_match must be 'case_insensitive' or 'exact' (got {:?})",
                other
            ),
        };
        let delimiter = opt_str(opts, "delimiter").unwrap_or(",");
        Ok(SpecSource {
            csv_paths: opt_seq_str(opts, "csv_paths")
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            name_column: opt_str(opts, "name_column")
                .unwrap_or(DEFAULT_NAME_COLUMN)
                .into(),
            values_column: opt_str(opts, "values_column")
                .unwrap_or(DEFAULT_VALUES_COLUMN)
                .into(),
            vector_column: opt_str(opts, "vector_column")
                .unwrap_or(DEFAULT_VECTOR_COLUMN)
                .into(),
            name_match,
            delimiter: delimiter.as_bytes().first().copied().unwrap_or(b','),
        })
    }

    /// Read every CSV. Missing / unreadable files and files lacking the
    /// configured columns are skipped silently; later files override
    /// earlier ones.
    pub fn load(&self) -> ValueSpecs {
        let mut map = HashMap::new();
        for path in &self.csv_paths {
            if !path.is_file() {
                continue;
            }
            let Ok(mut rdr) = csv::ReaderBuilder::new()
                .delimiter(self.delimiter)
                .has_headers(true)
                .flexible(true)
                .from_path(path)
            else {
                continue;
            };
            let headers = match rdr.headers() {
                Ok(h) => h.clone(),
                Err(_) => continue,
            };
            let name_col = headers.iter().position(|h| h == self.name_column);
            let values_col = headers.iter().position(|h| h == self.values_column);
            let (Some(name_col), Some(values_col)) = (name_col, values_col) else {
                continue;
            };
            for record in rdr.records().flatten() {
                let name = record.get(name_col).unwrap_or("").trim();
                let values_text = record.get(values_col).unwrap_or("").trim();
                if name.is_empty() || values_text.is_empty() {
                    continue;
                }
                let Some(spec) = parse_values(values_text) else {
                    continue;
                };
                for expanded in expand_name_range(name) {
                    map.insert(self.key(&expanded), spec.clone());
                }
            }
        }
        ValueSpecs {
            map,
            name_match: self.name_match,
        }
    }

    fn key(&self, name: &str) -> String {
        match self.name_match {
            NameMatch::Exact => name.to_string(),
            NameMatch::CaseInsensitive => name.to_lowercase(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ValueSpecs {
    map: HashMap<String, Spec>,
    name_match: NameMatch,
}

impl Default for ValueSpecs {
    fn default() -> Self {
        ValueSpecs {
            map: HashMap::new(),
            name_match: NameMatch::CaseInsensitive,
        }
    }
}

impl ValueSpecs {
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn lookup(&self, name: &str) -> Option<&Spec> {
        match self.name_match {
            NameMatch::Exact => self.map.get(name),
            NameMatch::CaseInsensitive => self.map.get(&name.to_lowercase()),
        }
    }
}

/// Expand a ranged catalog key into the names it covers:
///
/// * numeric suffix — `iM1a1-iM1a12` → `iM1a1` … `iM1a12`
/// * single-letter swap — `iI3aa-iI3ja` → `iI3aa`, `iI3ba`, … `iI3ja`
///
/// Anything else is returned unchanged.
pub fn expand_name_range(name: &str) -> Vec<String> {
    let parts: Vec<&str> = name.split('-').collect();
    let [first, last] = parts[..] else {
        return vec![name.to_string()];
    };
    if first.is_empty() || last.is_empty() {
        return vec![name.to_string()];
    }

    if let (Some((b1, n1)), Some((b2, n2))) =
        (split_numeric_suffix(first), split_numeric_suffix(last))
    {
        if b1 == b2 {
            if n2 < n1 {
                return vec![name.to_string()];
            }
            return (n1..=n2).map(|n| format!("{b1}{n}")).collect();
        }
    }

    let (a, b): (Vec<char>, Vec<char>) = (first.chars().collect(), last.chars().collect());
    if a.len() == b.len() {
        let diffs: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
        if let [pos] = diffs[..] {
            let (from, to) = (a[pos], b[pos]);
            if from.is_ascii_alphabetic() && to.is_ascii_alphabetic() && to >= from {
                return (from..=to)
                    .map(|c| {
                        let mut v = a.clone();
                        v[pos] = c;
                        v.into_iter().collect()
                    })
                    .collect();
            }
        }
    }

    vec![name.to_string()]
}

fn split_numeric_suffix(s: &str) -> Option<(&str, u64)> {
    let digits = s.bytes().rev().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits == s.len() {
        return None;
    }
    let (base, num) = s.split_at(s.len() - digits);
    Some((base, num.parse().ok()?))
}

static RE_RANGE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(-?\d+)-(-?\d+)$").unwrap());
static RE_DECIMAL_RANGE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(-?\d+(?:\.\d+)?)\s*-\s*(-?\d+(?:\.\d+)?)$").unwrap());
static RE_SET: Lazy<Regex> = Lazy::new(|| Regex::new(r"^-?\d+(?:\s*,\s*-?\d+)+$").unwrap());
static RE_RANGE_PAREN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(\d+)-(\d+)\s+\(([^)]+)\)$").unwrap());
static RE_RANGE_PLUS: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(\d+)-(\d+)\s*,\s*(.+)$").unwrap());

pub fn parse_values(text: &str) -> Option<Spec> {
    if text.to_ascii_lowercase().contains("yyyymmdd") {
        return Some(Spec {
            kind: SpecKind::Date,
        });
    }
    if let Some(c) = RE_RANGE.captures(text) {
        let lo: i64 = c[1].parse().ok()?;
        let hi: i64 = c[2].parse().ok()?;
        return Some(Spec {
            kind: SpecKind::Range(lo, hi),
        });
    }
    if let Some(c) = RE_DECIMAL_RANGE
        .captures(text)
        .filter(|_| text.contains('.'))
    {
        let lo: f64 = c[1].parse().ok()?;
        let hi: f64 = c[2].parse().ok()?;
        return Some(Spec {
            kind: SpecKind::Decimal(lo, hi),
        });
    }
    if RE_SET.is_match(text) {
        let vals: Vec<i64> = text
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        return Some(Spec {
            kind: SpecKind::Set(vals),
        });
    }
    if let Some(c) = RE_RANGE_PAREN.captures(text) {
        let lo: i64 = c[1].parse().ok()?;
        let hi: i64 = c[2].parse().ok()?;
        let mut vals: Vec<i64> = (lo..=hi).collect();
        for x in c[3].split(',') {
            if let Ok(n) = x.trim().parse::<i64>() {
                vals.push(n);
            }
        }
        return Some(Spec {
            kind: SpecKind::Set(vals),
        });
    }
    if let Some(c) = RE_RANGE_PLUS.captures(text) {
        let lo: i64 = c[1].parse().ok()?;
        let hi: i64 = c[2].parse().ok()?;
        let extras: Vec<i64> = c[3]
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        if extras.is_empty() {
            return None;
        }
        let mut vals: Vec<i64> = (lo..=hi).collect();
        vals.extend(extras);
        return Some(Spec {
            kind: SpecKind::Set(vals),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_decimal_ranges() {
        assert!(matches!(
            parse_values("0.5-40.0").map(|s| s.kind),
            Some(SpecKind::Decimal(lo, hi)) if lo == 0.5 && hi == 40.0
        ));
        assert!(matches!(
            parse_values("0-2").map(|s| s.kind),
            Some(SpecKind::Range(0, 2))
        ));
    }

    #[test]
    fn parses_yyyymmdd_dates() {
        assert!(matches!(
            parse_values("8 Characters: yyyymmdd").map(|s| s.kind),
            Some(SpecKind::Date)
        ));
    }

    #[test]
    fn expands_numeric_suffix_ranges() {
        assert_eq!(expand_name_range("x1-x3"), vec!["x1", "x2", "x3"]);
        assert_eq!(
            expand_name_range("iM1a9-iM1a11"),
            vec!["iM1a9", "iM1a10", "iM1a11"]
        );
    }

    #[test]
    fn expands_single_letter_ranges() {
        assert_eq!(expand_name_range("i3aa-i3ca"), vec!["i3aa", "i3ba", "i3ca"]);
    }

    #[test]
    fn leaves_plain_names_alone() {
        assert_eq!(expand_name_range("age"), vec!["age"]);
        assert_eq!(expand_name_range("a-b-c"), vec!["a-b-c"]);
        assert_eq!(expand_name_range("x3-x1"), vec!["x3-x1"]);
    }
}
