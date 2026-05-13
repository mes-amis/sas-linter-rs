use std::collections::BTreeMap;

use crate::config::{opt_bool, opt_map_str_str};
use crate::finding::{Finding, Severity};
use crate::rule::{CheckContext, Rule};

use super::RuleMeta;

pub struct EncodingIssues {
    use_defaults: bool,
    replacements: Vec<(Vec<u8>, Vec<u8>)>,
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
                replacements: opt_map_str_str(opts, "replacements")
                    .into_iter()
                    .map(|(k, v)| (k.into_bytes(), v.into_bytes()))
                    .collect(),
                autofix: opt_bool(opts, "autofix").unwrap_or(false),
            }))
        },
    }
}

/// Multibyte UTF-8 sequences for smart punctuation, byte-keyed so we
/// can pattern-match on the raw source byte stream regardless of its
/// declared Rust encoding.
fn utf8_replacements() -> &'static BTreeMap<&'static [u8], &'static [u8]> {
    static MAP: once_cell::sync::OnceCell<BTreeMap<&'static [u8], &'static [u8]>> =
        once_cell::sync::OnceCell::new();
    MAP.get_or_init(|| {
        let entries: &[(&[u8], &[u8])] = &[
            (b"\xE2\x80\x98", b"'"),
            (b"\xE2\x80\x99", b"'"),
            (b"\xE2\x80\x9A", b"'"),
            (b"\xE2\x80\x9B", b"'"),
            (b"\xE2\x80\x9C", b"\""),
            (b"\xE2\x80\x9D", b"\""),
            (b"\xE2\x80\x9E", b"\""),
            (b"\xE2\x80\x93", b"-"),
            (b"\xE2\x80\x94", b"-"),
            (b"\xE2\x80\x95", b"-"),
            (b"\xE2\x80\xA6", b"..."),
            (b"\xC2\xA0", b" "),
            (b"\xE2\x80\x80", b" "),
            (b"\xE2\x80\x81", b" "),
            (b"\xE2\x80\x82", b" "),
            (b"\xE2\x80\x83", b" "),
            (b"\xE2\x80\x84", b" "),
            (b"\xE2\x80\x85", b" "),
            (b"\xE2\x80\x86", b" "),
            (b"\xE2\x80\x87", b" "),
            (b"\xE2\x80\x88", b" "),
            (b"\xE2\x80\x89", b" "),
            (b"\xE2\x80\x8A", b" "),
            (b"\xE2\x80\xA8", b"\n"),
            (b"\xE2\x80\xA9", b"\n"),
            (b"\xC3\x90", b"-"),
            (b"\xC3\x92", b"\""),
            (b"\xC3\x93", b"\""),
            (b"\xC3\x94", b"'"),
            (b"\xC3\x95", b"'"),
        ];
        entries.iter().copied().collect()
    })
}

/// Single Win-1252 bytes only replaced when they are NOT part of a
/// valid UTF-8 sequence. 0x85 deliberately omitted — too noisy in
/// real corpora (corrupted Latin-1 letter inside surnames).
fn byte_replacements() -> &'static [(u8, &'static [u8])] {
    &[
        (0x82, b"'"),
        (0x91, b"'"),
        (0x92, b"'"),
        (0x93, b"\""),
        (0x94, b"\""),
        (0x96, b"-"),
        (0x97, b"-"),
        (0xA0, b" "),
    ]
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

    fn check(&self, ctx: &CheckContext) -> Vec<Finding> {
        let mut findings = Vec::new();
        if self.use_defaults {
            findings.extend(default_findings(ctx.source, ctx.path, self.autofix));
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
        let mut bytes = source.as_bytes().to_vec();
        // User replacements run FIRST so project-specific patterns can
        // target byte sequences the canonical defaults would otherwise
        // consume.
        for (from, to) in &self.replacements {
            bytes = replace_bytes(&bytes, from, to);
        }
        if self.use_defaults {
            bytes = apply_canonical_fix(&bytes);
        }
        // The transform may yield bytes that aren't valid UTF-8. The
        // downstream tokenizer requires UTF-8, so transcode as
        // Win-1252 → UTF-8 if needed; otherwise return as-is.
        match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => crate::encoding::decode_bytes(&e.into_bytes()),
        }
    }
}

fn replace_bytes(haystack: &[u8], needle: &[u8], replacement: &[u8]) -> Vec<u8> {
    if needle.is_empty() {
        return haystack.to_vec();
    }
    let mut out = Vec::with_capacity(haystack.len());
    let mut i = 0;
    while i + needle.len() <= haystack.len() {
        if &haystack[i..i + needle.len()] == needle {
            out.extend_from_slice(replacement);
            i += needle.len();
        } else {
            out.push(haystack[i]);
            i += 1;
        }
    }
    out.extend_from_slice(&haystack[i..]);
    out
}

fn apply_canonical_fix(source: &[u8]) -> Vec<u8> {
    let utf8 = utf8_replacements();
    let byte = byte_replacements();
    let mut out = Vec::with_capacity(source.len());
    let n = source.len();
    let mut i = 0;
    while i < n {
        let b = source[i];
        if b < 0x80 {
            out.push(b);
            i += 1;
            continue;
        }
        let seq_len = utf8_sequence_length(source, i);
        if seq_len > 0 {
            let seq = &source[i..i + seq_len];
            if let Some(replacement) = utf8.get(seq) {
                out.extend_from_slice(replacement);
            } else {
                out.extend_from_slice(seq);
            }
            i += seq_len;
        } else if let Some((_, repl)) = byte.iter().find(|(by, _)| *by == b) {
            out.extend_from_slice(repl);
            i += 1;
        } else {
            out.push(b);
            i += 1;
        }
    }
    out
}

fn default_findings(source: &str, path: &str, autofix: bool) -> Vec<Finding> {
    let utf8 = utf8_replacements();
    let byte = byte_replacements();
    let mut findings = Vec::new();
    let bytes = source.as_bytes();
    let n = bytes.len();
    let mut line: u32 = 1;
    let mut col: u32 = 1;
    let mut i = 0;
    while i < n {
        let b = bytes[i];
        if b < 0x80 {
            if b == 0x0A {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
            i += 1;
            continue;
        }
        let seq_len = utf8_sequence_length(bytes, i);
        if seq_len > 0 {
            let seq = &bytes[i..i + seq_len];
            if let Some(replacement) = utf8.get(seq) {
                let cp = std::str::from_utf8(seq)
                    .ok()
                    .and_then(|s| s.chars().next())
                    .map(|c| c as u32)
                    .unwrap_or(0);
                findings.push(Finding {
                    path: path.to_string(),
                    line,
                    column: col,
                    rule: ID,
                    message: format!(
                        "UTF-8 U+{:04X} -> {:?}{}",
                        cp,
                        std::str::from_utf8(replacement).unwrap_or(""),
                        if autofix { " (autofixed)" } else { "" }
                    ),
                    severity: Severity::Warning,
                });
            }
            col += 1;
            i += seq_len;
        } else if let Some((_, repl)) = byte.iter().find(|(by, _)| *by == b) {
            findings.push(Finding {
                path: path.to_string(),
                line,
                column: col,
                rule: ID,
                message: format!(
                    "Windows-1252 0x{:02X} -> {:?}{}",
                    b,
                    std::str::from_utf8(repl).unwrap_or(""),
                    if autofix { " (autofixed)" } else { "" }
                ),
                severity: Severity::Warning,
            });
            col += 1;
            i += 1;
        } else {
            col += 1;
            i += 1;
        }
    }
    findings
}

fn replacement_findings(
    source: &str,
    path: &str,
    replacements: &[(Vec<u8>, Vec<u8>)],
    autofix: bool,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (line_idx, line) in source.split_inclusive('\n').enumerate() {
        let chomped = line.trim_end_matches('\n').trim_end_matches('\r');
        for (from, to) in replacements {
            let mut search_from = 0;
            while let Some(idx) = find_subslice(&chomped.as_bytes()[search_from..], from) {
                let abs = search_from + idx;
                let col = chomped[..abs].chars().count() + 1;
                findings.push(Finding {
                    path: path.to_string(),
                    line: (line_idx + 1) as u32,
                    column: col as u32,
                    rule: ID,
                    message: if autofix {
                        format!(
                            "found {:?} -> {:?} (autofixed)",
                            String::from_utf8_lossy(from),
                            String::from_utf8_lossy(to)
                        )
                    } else {
                        format!("found {:?} (no autofix)", String::from_utf8_lossy(from))
                    },
                    severity: Severity::Warning,
                });
                search_from = abs + from.len();
            }
        }
    }
    findings
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

fn utf8_sequence_length(bytes: &[u8], i: usize) -> usize {
    let b = bytes[i];
    if b < 0xC2 {
        return 0;
    }
    if b < 0xE0 {
        return if valid_continuation(bytes, i + 1) {
            2
        } else {
            0
        };
    }
    if b < 0xF0 {
        return if valid_continuation(bytes, i + 1) && valid_continuation(bytes, i + 2) {
            3
        } else {
            0
        };
    }
    if b < 0xF5
        && valid_continuation(bytes, i + 1)
        && valid_continuation(bytes, i + 2)
        && valid_continuation(bytes, i + 3)
    {
        return 4;
    }
    0
}

fn valid_continuation(bytes: &[u8], i: usize) -> bool {
    if i >= bytes.len() {
        return false;
    }
    let b = bytes[i];
    (0x80..0xC0).contains(&b)
}
