use std::collections::HashSet;

use percent_encoding::percent_decode_str;

use crate::parser::NewEndpoint;

/// The parameter names whose truthy presence marks a line as bypassing TLS
/// verification; matched case-insensitively.
const INSECURE_PARAM_NAMES: [&str; 3] = ["allowinsecure", "allow_insecure", "insecure"];
const INSECURE_PARAM_VALUES: [&str; 3] = ["1", "true", "yes"];

// ---------------------------------------------------------------------------
// HTML entity unescaping (minimal)
// ---------------------------------------------------------------------------

/// Replaces the five named entities that appear in URL parameter lists
/// (`&amp;` `&lt;` `&gt;` `&quot;` `&apos;`) plus the numeric decimal
/// (`&#59;`) and hex (`&#x3B;`) forms; anything else stays literal.
pub(super) fn html_unescape(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'&' {
            if let Some((decoded, consumed)) = decode_entity(&input[index..]) {
                match decoded {
                    Decoded::Str(text) => out.push_str(text),
                    Decoded::Char(ch) => out.push(ch),
                }
                index += consumed;
                continue;
            }
        }
        let ch = input[index..].chars().next().unwrap_or('&');
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

enum Decoded {
    Str(&'static str),
    Char(char),
}

/// The entity starting at `slice[0] == '&'`: the decoded value plus the
/// number of bytes consumed (including the `;`).
fn decode_entity(slice: &str) -> Option<(Decoded, usize)> {
    let semicolon = slice.find(';')?;
    if semicolon > 10 {
        return None;
    }
    let body = &slice[1..semicolon];
    let decoded = match body {
        "amp" => Decoded::Str("&"),
        "lt" => Decoded::Str("<"),
        "gt" => Decoded::Str(">"),
        "quot" => Decoded::Str("\""),
        "apos" => Decoded::Str("'"),
        _ => {
            let digits = body.strip_prefix('#')?;
            let code = if let Some(hex) =
                digits.strip_prefix('x').or_else(|| digits.strip_prefix('X'))
            {
                u32::from_str_radix(hex, 16).ok()?
            } else {
                digits.parse::<u32>().ok()?
            };
            Decoded::Char(char::from_u32(code)?)
        }
    };
    Some((decoded, semicolon + 1))
}

// ---------------------------------------------------------------------------
// Insecure-line detection
// ---------------------------------------------------------------------------

/// Whether a subscription line carries an `allowInsecure=1`-style
/// parameter. The scan runs on the HTML-unescaped line and again on its
/// percent-decoded form: decoding mangles separator leftovers (`%Ba` is a
/// valid escape that eats the parameter name's first letter), so the
/// raw `3%b`/`%3b` boundary is only visible before it. A parameter
/// starting at a `?`/`&`/`;` boundary (the encoded-semicolon leftovers
/// count as one) whose name is an insecure variant and whose value is
/// truthy, terminated like a real query parameter, marks the line.
pub(super) fn is_insecure_line(raw: &str) -> bool {
    let decoded = percent_decode_str(raw).decode_utf8_lossy().into_owned();
    has_insecure_parameter(&html_unescape(raw).to_lowercase())
        || has_insecure_parameter(&html_unescape(&decoded).to_lowercase())
}

fn has_insecure_parameter(processed: &str) -> bool {
    let bytes = processed.as_bytes();
    for name in INSECURE_PARAM_NAMES {
        let mut from = 0;
        while let Some(found) = find_from(processed, name, from) {
            let after = found + name.len();
            if has_boundary_before(bytes, found)
                && bytes.get(after) == Some(&b'=')
                && matches_insecure_value(bytes, after + 1)
            {
                return true;
            }
            from = found + 1;
        }
    }
    false
}

fn find_from(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    if from >= haystack.len() {
        return None;
    }
    haystack[from..].find(needle).map(|offset| from + offset)
}

/// The byte(s) right before the parameter name: one of the separators, or
/// the encoded-semicolon leftover.
fn has_boundary_before(bytes: &[u8], position: usize) -> bool {
    if position == 0 {
        return false;
    }
    match bytes[position - 1] {
        b'?' | b'&' | b';' => true,
        _ => position >= 3 && matches!(&bytes[position - 3..position], b"3%b" | b"%3b"),
    }
}

fn matches_insecure_value(bytes: &[u8], start: usize) -> bool {
    for value in INSECURE_PARAM_VALUES {
        let end = start + value.len();
        if bytes.len() >= end && &bytes[start..end] == value.as_bytes() {
            return match bytes.get(end) {
                None => true,
                Some(&terminator) => {
                    matches!(terminator, b'&' | b';' | b'#') || terminator.is_ascii_whitespace()
                }
            };
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Line filtering & endpoint dedup
// ---------------------------------------------------------------------------

/// Drops the insecure lines, keeping the rest verbatim; returns the kept
/// content and the removed count.
pub(super) fn filter_lines(content: &str) -> (String, usize) {
    let mut kept: Vec<&str> = Vec::new();
    let mut removed = 0usize;
    for line in content.lines() {
        if is_insecure_line(line) {
            removed += 1;
        } else {
            kept.push(line);
        }
    }
    (kept.join("\n"), removed)
}

/// First pass dedups by the exact raw link, the second by
/// (lowercased server, port) — but only endpoints that have both a server
/// and a port; the first occurrence wins and the order is preserved.
pub(super) fn dedup_endpoints(endpoints: Vec<NewEndpoint>) -> Vec<NewEndpoint> {
    let mut seen_raw = HashSet::new();
    let unique_raw: Vec<NewEndpoint> = endpoints
        .into_iter()
        .filter(|endpoint| seen_raw.insert(endpoint.raw.clone()))
        .collect();
    let mut seen_address = HashSet::new();
    unique_raw
        .into_iter()
        .filter(|endpoint| {
            endpoint.server.is_empty()
                || endpoint.server_port == 0
                || seen_address.insert((endpoint.server.to_lowercase(), endpoint.server_port))
        })
        .collect()
}
