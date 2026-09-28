// The shared test-URL normalizer (used by the sites/routing rule editor
// and the latency scans).

/// Validates and normalizes a user-entered test URL: http/https scheme,
/// non-empty host, no control characters, sane length. Returns the parsed
/// (normalized) form used for storage and deduplication.
pub fn validate_test_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("URL must not be empty".into());
    }
    if trimmed.chars().any(char::is_control) {
        return Err("URL must not contain control characters".into());
    }
    if trimmed.len() > 2048 {
        return Err("URL is too long (max 2048 characters)".into());
    }
    // bare "youtube.com" style input is fine — https:// is assumed whenever
    // no scheme is spelled out (prepending avoids `host:port` being read as
    // a scheme by the parser)
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let parsed = url::Url::parse(&candidate).map_err(|_| "this does not look like a valid URL".to_string())?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(format!("URL scheme must be http or https, got {other:?}")),
    }
    match parsed.host_str() {
        Some(host) if !host.is_empty() => {}
        _ => return Err("URL must include a host".into()),
    }
    Ok(parsed.to_string())
}
