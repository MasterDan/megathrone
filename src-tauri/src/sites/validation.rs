use crate::settings::validate_test_url;

/// Rule types a category member can be. Everything but `url` is a sing-box
/// domain matcher verbatim; `url` is a full test URL that routes by its
/// hostname (as a domain suffix).
pub const RULE_TYPE_URL: &str = "url";
pub const RULE_TYPE_DOMAIN_SUFFIX: &str = "domain_suffix";
pub const RULE_TYPE_DOMAIN: &str = "domain";
pub const RULE_TYPE_KEYWORD: &str = "domain_keyword";
pub const RULE_TYPE_REGEX: &str = "domain_regex";
pub const RULE_TYPES: [&str; 5] = [
    RULE_TYPE_DOMAIN_SUFFIX,
    RULE_TYPE_DOMAIN,
    RULE_TYPE_KEYWORD,
    RULE_TYPE_REGEX,
    RULE_TYPE_URL,
];

/// The URL a rule is probed with, when the rule type can be probed at all:
/// `url` rules are fetched as-is, bare-domain rules as `https://<value>/`.
/// Keyword/regex rules match too broadly to build a probe target and join
/// routing only.
pub fn probe_url(rule_type: &str, value: &str) -> Option<String> {
    match rule_type {
        RULE_TYPE_URL => Some(value.to_string()),
        RULE_TYPE_DOMAIN_SUFFIX | RULE_TYPE_DOMAIN => Some(format!("https://{value}/")),
        _ => None,
    }
}

pub(crate) fn validate_rule_type(value: &str) -> Result<String, String> {
    if RULE_TYPES.contains(&value) {
        Ok(value.to_string())
    } else {
        Err(format!("unknown rule type `{value}` (expected one of: {RULE_TYPES:?})"))
    }
}

/// Validates and normalizes a rule value for its type. Returns the form
/// used for storage and deduplication.
pub(crate) fn validate_rule_value(rule_type: &str, raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("the rule value must not be empty".into());
    }
    if trimmed.chars().any(char::is_control) {
        return Err("the rule value must not contain control characters".into());
    }
    match rule_type {
        RULE_TYPE_URL => validate_test_url(trimmed),
        RULE_TYPE_DOMAIN_SUFFIX | RULE_TYPE_DOMAIN => {
            let lowered = trimmed.to_lowercase();
            if lowered.len() > 253 {
                return Err("the domain is too long (max 253 characters)".into());
            }
            if lowered
                .chars()
                .any(|c| matches!(c, '/' | ':' | '?' | '#' | '@' | ' ' | '\t'))
            {
                return Err(
                    "a domain rule must be a bare host like `example.com` — for full URLs use the URL type".into(),
                );
            }
            Ok(lowered)
        }
        RULE_TYPE_KEYWORD | RULE_TYPE_REGEX => {
            if trimmed.len() > 256 {
                return Err("the pattern is too long (max 256 characters)".into());
            }
            Ok(trimmed.to_string())
        }
        other => Err(format!("unknown rule type `{other}`")),
    }
}

/// The hostname part of a stored `url`-typed rule — what it routes by.
pub(crate) fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
        .unwrap_or_else(|| url.trim().to_string())
}
