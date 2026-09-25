use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;

/// uTLS fingerprints sing-box accepts. Links in the wild carry junk values
/// (e.g. `fp=unsafe`) that would make sing-box reject the whole config, so
/// unknown fingerprints are dropped (uTLS falls back to its default).
pub const UTLS_FINGERPRINTS: &[&str] = &[
    "chrome",
    "firefox",
    "edge",
    "safari",
    "360",
    "qq",
    "ios",
    "android",
    "random",
    "randomized",
];

/// Lowercases a link's `fp` value and keeps it only when sing-box knows it.
pub fn normalize_fingerprint(raw: &str) -> Option<String> {
    let fingerprint = raw.trim().to_ascii_lowercase();
    UTLS_FINGERPRINTS
        .contains(&fingerprint.as_str())
        .then_some(fingerprint)
}

pub(super) fn decode_base64(input: &str) -> Option<Vec<u8>> {
    [
        STANDARD.decode(input).ok(),
        STANDARD_NO_PAD.decode(input).ok(),
        URL_SAFE.decode(input).ok(),
        URL_SAFE_NO_PAD.decode(input).ok(),
    ]
    .into_iter()
    .flatten()
    .next()
}
