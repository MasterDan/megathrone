// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// ciadpi short options the app owns (listener / process management). Both
/// `-p 1080` and the glued `-p1080` form are getopt-valid, so any short token
/// *starting* with these letters is rejected.
const FORBIDDEN_SHORT: [char; 5] = ['p', 'i', 'D', 'w', 'E'];
/// …and their long forms (`--port=…` etc.).
const FORBIDDEN_LONG: [&str; 5] = ["port", "ip", "daemon", "pidfile", "transparent"];

const MAX_STRATEGY_CHARS: usize = 2048;
pub(super) const MAX_STRATEGY_TOKENS: usize = 64;
pub(super) const MAX_NAME_CHARS: usize = 64;

/// Validates a user-entered strategy name.
pub fn validate_name(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("name must not be empty".into());
    }
    if trimmed.chars().any(char::is_control) {
        return Err("name must not contain control characters".into());
    }
    if trimmed.chars().count() > MAX_NAME_CHARS {
        return Err(format!("name is too long (max {MAX_NAME_CHARS} characters)"));
    }
    Ok(trimmed.to_string())
}

/// Validates a strategy argument line and splits it into argv entries. The
/// process is spawned without a shell, so no shell-quoting hazards exist —
/// but the app-managed options (listen address/port, daemonizing, pidfile,
/// transparent mode) must not appear in the user line.
pub fn parse_strategy_args(raw: &str) -> Result<Vec<String>, String> {
    let trimmed = raw.trim();
    if trimmed.chars().count() > MAX_STRATEGY_CHARS {
        return Err(format!("argument line is too long (max {MAX_STRATEGY_CHARS} characters)"));
    }
    let tokens: Vec<String> = trimmed.split_whitespace().map(str::to_string).collect();
    if tokens.len() > MAX_STRATEGY_TOKENS {
        return Err(format!("too many arguments (max {MAX_STRATEGY_TOKENS})"));
    }
    for token in &tokens {
        if let Some(long) = token.strip_prefix("--") {
            if long.is_empty() {
                return Err("bare `--` is not a valid argument".into());
            }
            let name = long.split('=').next().unwrap_or(long);
            if FORBIDDEN_LONG.contains(&name) {
                return Err(format!(
                    "`{token}` conflicts with an option managed by the app \
                     (listen address/port, daemonizing, pidfile, transparent mode)"
                ));
            }
        } else if let Some(short) = token.strip_prefix('-') {
            let Some(first) = short.chars().next() else {
                return Err("bare `-` is not a valid argument".into());
            };
            if FORBIDDEN_SHORT.contains(&first) {
                return Err(format!(
                    "`{token}` conflicts with an option managed by the app \
                     (listen address/port, daemonizing, pidfile, transparent mode)"
                ));
            }
        }
    }
    Ok(tokens)
}
