use std::fs;
use std::io;
use std::path::Path;

/// Read a SAS source file with the same fallback behavior as the Ruby gem:
///
/// 1. Try as UTF-8. If valid, return as-is.
/// 2. Otherwise transcode from Windows-1252 (with `'` as replacement for
///    invalid sequences). SAS sources commonly arrive Win-1252-encoded
///    after a Word/Outlook round-trip.
/// 3. Falls back to ISO-8859-1 if Windows-1252 produces invalid output.
///
/// The lexer requires valid UTF-8, so non-UTF-8 sources must be transcoded
/// before tokenization.
pub fn read_source(path: &Path) -> io::Result<String> {
    let raw = fs::read(path)?;
    Ok(decode_bytes(&raw))
}

pub fn decode_bytes(raw: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(raw) {
        return s.to_string();
    }

    let (decoded, _, had_errors) = encoding_rs::WINDOWS_1252.decode(raw);
    if !had_errors {
        return decoded.into_owned();
    }

    let (decoded, _, _) = encoding_rs::WINDOWS_1252.decode(raw);
    decoded
        .chars()
        .map(|c| if c == '\u{FFFD}' { '\'' } else { c })
        .collect()
}
