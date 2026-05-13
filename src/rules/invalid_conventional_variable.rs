use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::OnceLock;

use regex::Regex;
use sas_lexer::TokenType;

use crate::config::{opt_seq_str, opt_str};
use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};

use super::RuleMeta;

const ID: &str = "invalid_conventional_variable";
const DESCRIPTION: &str = "Identifier matches a configured naming convention but \
                            is absent from the known catalog.";
const DEFAULT_NAME_COLUMN: &str = "Name";
const MAX_EDIT_DISTANCE: usize = 4;
const MAX_SUGGESTIONS: usize = 3;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum NameMatch {
    CaseInsensitive,
    Exact,
}

#[derive(Default)]
struct Catalog {
    keys: HashSet<String>,
    originals: HashMap<String, String>,
}

pub struct InvalidConventionalVariable {
    pattern: Option<Regex>,
    csv_paths: Vec<PathBuf>,
    name_column: String,
    name_match: NameMatch,
    delimiter: u8,
    allow_strings: HashSet<String>,
    allow_patterns: Vec<Regex>,
    catalog: OnceLock<Catalog>,
}

pub fn meta() -> RuleMeta {
    RuleMeta {
        id: ID,
        description: DESCRIPTION,
        supports_autofix: false,
        default_factory: || {
            Box::new(InvalidConventionalVariable {
                pattern: None,
                csv_paths: Vec::new(),
                name_column: DEFAULT_NAME_COLUMN.into(),
                name_match: NameMatch::CaseInsensitive,
                delimiter: b',',
                allow_strings: HashSet::new(),
                allow_patterns: Vec::new(),
                catalog: OnceLock::new(),
            })
        },
        config_factory: |opts| {
            let pattern = match opt_str(opts, "pattern") {
                Some(p) if !p.is_empty() => Some(Regex::new(p).map_err(|e| {
                    anyhow::anyhow!("invalid_conventional_variable pattern: {}", e)
                })?),
                _ => None,
            };
            let name_match = match opt_str(opts, "name_match").unwrap_or("case_insensitive") {
                "case_insensitive" => NameMatch::CaseInsensitive,
                "exact" => NameMatch::Exact,
                other => anyhow::bail!(
                    "name_match must be 'case_insensitive' or 'exact' (got {:?})",
                    other
                ),
            };
            let delimiter = opt_str(opts, "delimiter").unwrap_or(",");
            let delimiter = delimiter.as_bytes().first().copied().unwrap_or(b',');
            let csv_paths = opt_seq_str(opts, "csv_paths")
                .into_iter()
                .map(PathBuf::from)
                .collect();
            let (allow_strings, allow_patterns) =
                parse_allow_list(opt_seq_str(opts, "allow_list"), name_match)?;
            Ok(Box::new(InvalidConventionalVariable {
                pattern,
                csv_paths,
                name_column: opt_str(opts, "name_column")
                    .unwrap_or(DEFAULT_NAME_COLUMN)
                    .into(),
                name_match,
                delimiter,
                allow_strings,
                allow_patterns,
                catalog: OnceLock::new(),
            }))
        },
    }
}

impl Rule for InvalidConventionalVariable {
    fn id(&self) -> &'static str {
        ID
    }
    fn description(&self) -> &'static str {
        DESCRIPTION
    }
    fn severity(&self) -> Severity {
        Severity::Error
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        let Some(pattern) = &self.pattern else {
            return Vec::new();
        };
        let catalog = self.catalog();
        if catalog.keys.is_empty() {
            return Vec::new();
        }

        let mut findings = Vec::new();
        let mut suggestion_cache: HashMap<String, Vec<String>> = HashMap::new();
        for tok in &ctx.tokens.default {
            if tok.token_type != TokenType::Identifier {
                continue;
            }
            if !pattern.is_match(&tok.text) {
                continue;
            }
            if self.is_known(&tok.text, catalog) {
                continue;
            }

            let suggestions = suggestion_cache
                .entry(tok.text.clone())
                .or_insert_with(|| self.closest_matches(&tok.text, catalog));
            findings.push(Finding {
                path: ctx.path.to_string(),
                line: tok.start_line,
                column: tok.start_column + 1,
                rule: ID,
                message: build_message(&tok.text, suggestions),
                severity: Severity::Error,
            });
        }
        findings
    }
}

impl InvalidConventionalVariable {
    fn catalog(&self) -> &Catalog {
        self.catalog.get_or_init(|| self.load_catalog())
    }

    fn load_catalog(&self) -> Catalog {
        let mut keys = HashSet::new();
        let mut originals = HashMap::new();
        for path in &self.csv_paths {
            if !path.is_file() {
                continue;
            }
            let Ok(mut rdr) = csv::ReaderBuilder::new()
                .delimiter(self.delimiter)
                .has_headers(true)
                .from_path(path)
            else {
                continue;
            };
            let headers = match rdr.headers() {
                Ok(h) => h.clone(),
                Err(_) => continue,
            };
            let Some(name_col) = headers.iter().position(|h| h == self.name_column) else {
                continue;
            };
            for record in rdr.records().flatten() {
                let name = record.get(name_col).unwrap_or("").trim();
                if name.is_empty() {
                    continue;
                }
                let key = self.normalize(name);
                keys.insert(key.clone());
                originals.insert(key, name.to_string());
            }
        }
        Catalog { keys, originals }
    }

    fn normalize(&self, name: &str) -> String {
        match self.name_match {
            NameMatch::Exact => name.to_string(),
            NameMatch::CaseInsensitive => name.to_lowercase(),
        }
    }

    fn is_known(&self, name: &str, catalog: &Catalog) -> bool {
        let key = self.normalize(name);
        if catalog.keys.contains(&key) {
            return true;
        }
        if self.allow_strings.contains(&key) {
            return true;
        }
        self.allow_patterns.iter().any(|rx| rx.is_match(name))
    }

    fn closest_matches(&self, name: &str, catalog: &Catalog) -> Vec<String> {
        let key = self.normalize(name);
        let len = key.chars().count();
        let mut scored: Vec<(&String, usize)> = catalog
            .keys
            .iter()
            .filter(|k| k.chars().count().abs_diff(len) <= MAX_EDIT_DISTANCE)
            .map(|k| (k, levenshtein(&key, k)))
            .filter(|(_, d)| *d <= MAX_EDIT_DISTANCE)
            .collect();
        scored.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(b.0)));
        scored
            .into_iter()
            .take(MAX_SUGGESTIONS)
            .filter_map(|(k, _)| catalog.originals.get(k).cloned())
            .collect()
    }
}

fn parse_allow_list(
    entries: Vec<String>,
    name_match: NameMatch,
) -> anyhow::Result<(HashSet<String>, Vec<Regex>)> {
    let mut strings = HashSet::new();
    let mut patterns = Vec::new();
    for entry in entries {
        if entry.len() >= 2 && entry.starts_with('/') && entry.ends_with('/') {
            let body = &entry[1..entry.len() - 1];
            let rx = Regex::new(body).map_err(|e| {
                anyhow::anyhow!(
                    "invalid_conventional_variable allow_list regex {:?}: {}",
                    entry,
                    e
                )
            })?;
            patterns.push(rx);
        } else {
            let key = match name_match {
                NameMatch::Exact => entry,
                NameMatch::CaseInsensitive => entry.to_lowercase(),
            };
            strings.insert(key);
        }
    }
    Ok((strings, patterns))
}

// Wagner-Fischer DP, O(m*n) time, O(n) space.
#[allow(clippy::needless_range_loop)]
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (m, n) = (a.len(), b.len());
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr: Vec<usize> = vec![0; n + 1];
    for i in 0..m {
        curr[0] = i + 1;
        for j in 0..n {
            let cost = if a[i] == b[j] { 0 } else { 1 };
            curr[j + 1] = (curr[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}

fn build_message(name: &str, suggestions: &[String]) -> String {
    if suggestions.is_empty() {
        format!(
            "\"{}\" matches the configured naming convention but is not in the catalog.",
            name
        )
    } else {
        let quoted: Vec<String> = suggestions.iter().map(|s| format!("\"{}\"", s)).collect();
        format!(
            "\"{}\" matches the configured naming convention but is not in the catalog; \
             did you mean {}?",
            name,
            quoted.join(" or ")
        )
    }
}
