use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Parsed YAML config:
///
///   rules:
///     <rule_id>:
///       enabled: true | false   # default: true
///       autofix: true | false   # default: false
///       <rule-specific options>
///   format:
///     keywords: preserve | upper | lower
///     operator_spacing: true | false
///     indent_width: 2
///     align_comment_banners: true | false   # default: true
///     align_assignments: true | false       # default: true
///
/// Rules omitted from the config default to enabled with no options, so
/// adding a new rule never silently disables it for existing users — same
/// behavior as the Ruby gem.
#[derive(Debug, Default, Deserialize, Clone)]
pub struct Config {
    #[serde(default)]
    pub rules: BTreeMap<String, serde_yaml::Value>,
    #[serde(default)]
    pub format: FormatConfig,
}

#[derive(Debug, Default, Deserialize, Clone)]
pub struct FormatConfig {
    #[serde(default)]
    pub keywords: Option<String>,
    #[serde(default)]
    pub operator_spacing: Option<bool>,
    #[serde(default)]
    pub indent_width: Option<i64>,
    #[serde(default)]
    pub align_comment_banners: Option<bool>,
    #[serde(default)]
    pub align_assignments: Option<bool>,
}

impl Config {
    /// Load a YAML config file. Missing file returns the default (empty)
    /// config — matches Ruby's behavior.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config file {}", path.display()))?;
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        let cfg: Config = serde_yaml::from_str(&text)
            .with_context(|| format!("parsing YAML config at {}", path.display()))?;
        Ok(cfg)
    }

    /// Walk every rule entry and strip `autofix: true` — used by `--no-autofix`
    /// to guarantee a dry run cannot rewrite a file even if config opts in.
    pub fn strip_autofix(&mut self) {
        for opts in self.rules.values_mut() {
            if let serde_yaml::Value::Mapping(map) = opts {
                if let Some(v) = map.get_mut(serde_yaml::Value::String("autofix".into())) {
                    if v.as_bool() == Some(true) {
                        *v = serde_yaml::Value::Bool(false);
                    }
                }
            }
        }
    }
}

/// Helper for rules that take their own config sub-map.
pub fn opt_bool(opts: &serde_yaml::Value, key: &str) -> Option<bool> {
    opts.get(key).and_then(|v| v.as_bool())
}

pub fn opt_i64(opts: &serde_yaml::Value, key: &str) -> Option<i64> {
    opts.get(key).and_then(|v| v.as_i64())
}

pub fn opt_str<'a>(opts: &'a serde_yaml::Value, key: &str) -> Option<&'a str> {
    opts.get(key).and_then(|v| v.as_str())
}

pub fn opt_seq_str(opts: &serde_yaml::Value, key: &str) -> Vec<String> {
    opts.get(key)
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

pub fn opt_map_str_str(opts: &serde_yaml::Value, key: &str) -> Vec<(String, String)> {
    opts.get(key)
        .and_then(|v| v.as_mapping())
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| {
                    let k = k.as_str()?.to_string();
                    let v = v.as_str()?.to_string();
                    Some((k, v))
                })
                .collect()
        })
        .unwrap_or_default()
}
