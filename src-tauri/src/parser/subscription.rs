use std::collections::HashMap;

use serde_json::Value;

use super::links::parse_link;
use super::utils::decode_base64;

/// A parsed proxy endpoint: what we show in the UI plus the sing-box outbound
/// it maps to (so "connect to this" is a matter of feeding `outbound_json`
/// into a generated config later on).
#[derive(Debug, Clone)]
pub struct NewEndpoint {
    pub tag: String,
    pub protocol: String,
    pub server: String,
    pub server_port: u16,
    pub raw: String,
    pub outbound_json: String,
}

#[derive(Debug, Default)]
pub struct ParsedProfile {
    pub endpoints: Vec<NewEndpoint>,
    pub skipped: usize,
}

/// Proxy outbound types that can be dialed; group/system outbounds are skipped.
const PROXY_TYPES: &[&str] = &[
    "shadowsocks",
    "vmess",
    "vless",
    "trojan",
    "hysteria",
    "hysteria2",
    "tuic",
    "anytls",
    "shadowtls",
    "socks",
    "http",
    "naive",
    "ssh",
    "wireguard",
];

pub fn parse_subscription(content: &str) -> ParsedProfile {
    let content = content.trim();

    // Native sing-box config
    if content.starts_with('{') {
        if let Ok(cfg) = serde_json::from_str::<Value>(content) {
            if let Some(outbounds) = cfg.get("outbounds").and_then(Value::as_array) {
                return from_singbox_config(outbounds);
            }
        }
    }

    if content.contains("://") {
        return from_links(content);
    }

    // Whole-payload base64 subscription
    let compact: String = content.chars().filter(|c| !c.is_whitespace()).collect();
    if let Some(decoded) = decode_base64(&compact) {
        if let Ok(text) = String::from_utf8(decoded) {
            if text.contains("://") {
                return from_links(&text);
            }
        }
    }

    ParsedProfile {
        endpoints: Vec::new(),
        skipped: non_empty_lines(content),
    }
}

fn from_links(content: &str) -> ParsedProfile {
    let mut endpoints = Vec::new();
    let mut skipped = 0usize;
    let mut seen_tags: HashMap<String, usize> = HashMap::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }

        // Names may contain raw spaces, so try the whole line first and only
        // fall back to whitespace-splitting (several links on one line).
        let parsed = parse_link(line).or_else(|| line.split_whitespace().find_map(parse_link));

        match parsed {
            Some(mut endpoint) => {
                if endpoint.tag.is_empty() {
                    endpoint.tag = format!("{}:{}", endpoint.server, endpoint.server_port);
                }
                let count = seen_tags.entry(endpoint.tag.clone()).or_insert(0);
                *count += 1;
                if *count > 1 {
                    endpoint.tag = format!("{} #{}", endpoint.tag, count);
                }
                endpoints.push(endpoint);
            }
            None => skipped += 1,
        }
    }

    ParsedProfile { endpoints, skipped }
}

fn from_singbox_config(outbounds: &[Value]) -> ParsedProfile {
    let mut endpoints = Vec::new();
    let mut skipped = 0usize;

    for outbound in outbounds {
        let protocol = outbound.get("type").and_then(Value::as_str).unwrap_or_default();
        if !PROXY_TYPES.contains(&protocol) {
            skipped += 1;
            continue;
        }
        let raw = serde_json::to_string(outbound).unwrap_or_default();
        endpoints.push(NewEndpoint {
            tag: outbound.get("tag").and_then(Value::as_str).unwrap_or_default().to_string(),
            protocol: protocol.to_string(),
            server: outbound
                .get("server")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            server_port: outbound.get("server_port").and_then(Value::as_u64).unwrap_or(0) as u16,
            raw: raw.clone(),
            outbound_json: raw,
        });
    }

    ParsedProfile { endpoints, skipped }
}

fn non_empty_lines(content: &str) -> usize {
    content.lines().filter(|line| !line.trim().is_empty()).count()
}
