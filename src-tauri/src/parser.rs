use std::collections::HashMap;

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use percent_encoding::percent_decode_str;
use serde_json::{json, Map, Value};
use url::Url;

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

fn parse_link(raw: &str) -> Option<NewEndpoint> {
    let (scheme, _) = raw.split_once("://")?;
    match scheme.to_ascii_lowercase().as_str() {
        "vless" => parse_generic(raw, "vless"),
        "trojan" => parse_generic(raw, "trojan"),
        "hysteria2" | "hy2" => parse_generic(raw, "hysteria2"),
        "tuic" => parse_generic(raw, "tuic"),
        "socks5" | "socks" => parse_generic(raw, "socks"),
        "http" | "https" => parse_generic(raw, "http"),
        "ss" => parse_shadowsocks(raw),
        "vmess" => parse_vmess(raw),
        _ => None, // ssr and other exotic formats are intentionally skipped
    }
}

/// Parses URL-style share links: `scheme://userinfo@host:port?params#name`
fn parse_generic(raw: &str, protocol: &str) -> Option<NewEndpoint> {
    let url = Url::parse(&normalize_query(raw)).ok()?;
    let host = url.host_str()?.to_string();
    let port = url.port().unwrap_or(443);

    let params: HashMap<String, String> = url.query_pairs().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let tag = url.fragment().map(decode_fragment).unwrap_or_default();
    let username = percent_decode(url.username());
    let password = url.password().map(percent_decode).unwrap_or_default();

    let mut outbound = Map::new();
    outbound.insert("type".into(), json!(protocol));
    outbound.insert("tag".into(), json!(tag));
    outbound.insert("server".into(), json!(host));
    outbound.insert("server_port".into(), json!(port));

    match protocol {
        "vless" => {
            outbound.insert("uuid".into(), json!(username));
            if let Some(flow) = non_empty(params.get("flow")) {
                outbound.insert("flow".into(), json!(flow));
            }
            build_tls(&params, false, &mut outbound);
            build_transport(&params, &mut outbound);
        }
        "trojan" => {
            outbound.insert("password".into(), json!(username));
            build_tls(&params, true, &mut outbound);
            build_transport(&params, &mut outbound);
        }
        "hysteria2" => {
            outbound.insert("password".into(), json!(username));
            if let Some(obfs) = non_empty(params.get("obfs")) {
                outbound.insert(
                    "obfs".into(),
                    json!({
                        "type": obfs,
                        "password": params.get("obfs-password").cloned().unwrap_or_default(),
                    }),
                );
            }
            build_tls(&params, true, &mut outbound);
        }
        "tuic" => {
            outbound.insert("uuid".into(), json!(username));
            outbound.insert("password".into(), json!(password));
            if let Some(cc) = non_empty(params.get("congestion_control")) {
                outbound.insert("congestion_control".into(), json!(cc));
            }
            if let Some(mode) = non_empty(params.get("udp_relay_mode")) {
                outbound.insert("udp_relay_mode".into(), json!(mode));
            }
            build_tls(&params, true, &mut outbound);
        }
        "socks" => {
            outbound.insert("version".into(), json!("5"));
            if !username.is_empty() {
                outbound.insert("username".into(), json!(username));
                outbound.insert("password".into(), json!(password));
            }
        }
        "http" => {
            if !username.is_empty() {
                outbound.insert("username".into(), json!(username));
                outbound.insert("password".into(), json!(password));
            }
        }
        _ => unreachable!(),
    }

    let tag = outbound.get("tag").and_then(Value::as_str).unwrap_or_default().to_string();
    Some(NewEndpoint {
        tag,
        protocol: protocol.to_string(),
        server: host,
        server_port: port,
        raw: raw.to_string(),
        outbound_json: Value::Object(outbound).to_string(),
    })
}

fn parse_shadowsocks(raw: &str) -> Option<NewEndpoint> {
    let rest = raw.strip_prefix("ss://")?;
    let (body_with_query, fragment) = match rest.split_once('#') {
        Some((body, fragment)) => (body, Some(fragment)),
        None => (rest, None),
    };
    let (body, _query) = match body_with_query.split_once('?') {
        Some((body, query)) => (body, Some(query)),
        None => (body_with_query, None),
    };
    let tag = fragment.map(decode_fragment).unwrap_or_default();

    // Two shapes: `base64(method:pass)@host:port` (or plain `method:pass@host:port`)
    // and a fully encoded `base64(method:pass@host:port)`.
    let (method, password, host, port) = if let Some((userinfo, hostport)) = body.rsplit_once('@') {
        let credentials = decode_base64_to_string(userinfo)
            .map(|s| s.to_string())
            .unwrap_or_else(|| percent_decode(userinfo));
        let (method, password) = credentials.split_once(':')?;
        let (host, port) = split_host_port(hostport)?;
        (method.to_string(), password.to_string(), host, port)
    } else {
        let decoded = decode_base64_to_string(body)?;
        let (credentials, hostport) = decoded.rsplit_once('@')?;
        let (method, password) = credentials.split_once(':')?;
        let (host, port) = split_host_port(hostport)?;
        (method.to_string(), password.to_string(), host, port)
    };

    let outbound = json!({
        "type": "shadowsocks",
        "tag": tag,
        "server": host,
        "server_port": port,
        "method": method,
        "password": password,
    });
    let tag = outbound["tag"].as_str().unwrap_or_default().to_string();

    Some(NewEndpoint {
        tag,
        protocol: "shadowsocks".into(),
        server: host,
        server_port: port,
        raw: raw.to_string(),
        outbound_json: outbound.to_string(),
    })
}

fn parse_vmess(raw: &str) -> Option<NewEndpoint> {
    let payload = raw.strip_prefix("vmess://")?;
    let decoded = decode_base64_to_string(payload)?;
    let info: Value = serde_json::from_str(&decoded).ok()?;

    let server = info.get("add").and_then(Value::as_str)?.to_string();
    let port = info
        .get("port")
        .and_then(|p| p.as_u64().or_else(|| p.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(443) as u16;
    let uuid = info.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    let tag = info.get("ps").and_then(Value::as_str).unwrap_or_default().to_string();

    let mut outbound = Map::new();
    outbound.insert("type".into(), json!("vmess"));
    outbound.insert("tag".into(), json!(tag));
    outbound.insert("server".into(), json!(server));
    outbound.insert("server_port".into(), json!(port));
    outbound.insert("uuid".into(), json!(uuid));
    outbound.insert("security".into(), json!(info.get("scy").and_then(Value::as_str).unwrap_or("auto")));
    outbound.insert("alter_id".into(), json!(info.get("aid").and_then(Value::as_u64).unwrap_or(0)));

    let net = info.get("net").and_then(Value::as_str).unwrap_or("tcp");
    let params: HashMap<String, String> = [
        ("type".to_string(), net.to_string()),
        ("path".to_string(), info.get("path").and_then(Value::as_str).unwrap_or_default().to_string()),
        ("host".to_string(), info.get("host").and_then(Value::as_str).unwrap_or_default().to_string()),
        (
            "serviceName".to_string(),
            info.get("serviceName").and_then(Value::as_str).unwrap_or_default().to_string(),
        ),
        (
            "security".to_string(),
            info.get("tls").and_then(Value::as_str).unwrap_or_default().to_string(),
        ),
        ("sni".to_string(), info.get("sni").and_then(Value::as_str).unwrap_or_default().to_string()),
        ("fp".to_string(), info.get("fp").and_then(Value::as_str).unwrap_or_default().to_string()),
        (
            "alpn".to_string(),
            info.get("alpn").and_then(Value::as_str).unwrap_or_default().to_string(),
        ),
        (
            "insecure".to_string(),
            info.get("allowInsecure").and_then(Value::as_str).unwrap_or_default().to_string(),
        ),
    ]
    .into_iter()
    .filter(|(_, v)| !v.is_empty())
    .collect();

    build_tls(&params, false, &mut outbound);
    if net != "tcp" {
        build_transport(&params, &mut outbound);
    }

    let tag = outbound.get("tag").and_then(Value::as_str).unwrap_or_default().to_string();
    Some(NewEndpoint {
        tag,
        protocol: "vmess".into(),
        server,
        server_port: port,
        raw: raw.to_string(),
        outbound_json: Value::Object(outbound).to_string(),
    })
}

/// `security=tls` (or `reality`) turns TLS on; reality carries pbk/sid.
fn build_tls(params: &HashMap<String, String>, enabled_by_default: bool, outbound: &mut Map<String, Value>) {
    let security = params
        .get("security")
        .map(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(if enabled_by_default { "tls" } else { "none" });
    if security == "none" {
        return;
    }

    let sni = params
        .get("sni")
        .or_else(|| params.get("host"))
        .filter(|s| !s.is_empty());
    let alpn = params.get("alpn").filter(|s| !s.is_empty());
    let fingerprint = params.get("fp").filter(|s| !s.is_empty());
    let insecure = params
        .get("insecure")
        .or_else(|| params.get("allowInsecure"))
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);

    let mut tls = Map::new();
    tls.insert("enabled".into(), json!(true));
    if let Some(sni) = sni {
        tls.insert("server_name".into(), json!(sni));
    }
    if let Some(alpn) = alpn {
        tls.insert("alpn".into(), json!(alpn.split(',').collect::<Vec<_>>()));
    }
    if insecure {
        tls.insert("insecure".into(), json!(true));
    }
    if let Some(fingerprint) = fingerprint.and_then(|raw| normalize_fingerprint(raw)) {
        tls.insert("utls".into(), json!({ "enabled": true, "fingerprint": fingerprint }));
    }
    if security == "reality" {
        let mut reality = Map::new();
        reality.insert("enabled".into(), json!(true));
        if let Some(public_key) = params.get("pbk") {
            reality.insert("public_key".into(), json!(public_key));
        }
        if let Some(short_id) = params.get("sid") {
            reality.insert("short_id".into(), json!(short_id));
        }
        if tls.get("utls").is_none() {
            // sing-box requires a fingerprint for reality
            tls.insert("utls".into(), json!({ "enabled": true, "fingerprint": "chrome" }));
        }
        tls.insert("reality".into(), Value::Object(reality));
    }

    outbound.insert("tls".into(), Value::Object(tls));
}

/// `type`/net param selects the transport: ws / grpc / httpupgrade.
fn build_transport(params: &HashMap<String, String>, outbound: &mut Map<String, Value>) {
    let net = params
        .get("type")
        .or_else(|| params.get("net"))
        .map(|s| s.as_str())
        .unwrap_or("tcp");

    let path = params.get("path").cloned().unwrap_or_else(|| "/".to_string());
    let host = params.get("host").filter(|s| !s.is_empty());

    let transport = match net {
        "ws" => {
            let mut ws = Map::new();
            ws.insert("type".into(), json!("ws"));
            ws.insert("path".into(), json!(path));
            if let Some(host) = host {
                ws.insert("headers".into(), json!({ "Host": host }));
            }
            Value::Object(ws)
        }
        "grpc" => {
            let service = params
                .get("serviceName")
                .or_else(|| params.get("path"))
                .filter(|s| !s.is_empty());
            let mut grpc = Map::new();
            grpc.insert("type".into(), json!("grpc"));
            if let Some(service) = service {
                grpc.insert("service_name".into(), json!(service));
            }
            Value::Object(grpc)
        }
        "httpupgrade" => {
            let mut upgrade = Map::new();
            upgrade.insert("type".into(), json!("httpupgrade"));
            upgrade.insert("path".into(), json!(path));
            if let Some(host) = host {
                upgrade.insert("host".into(), json!(host));
            }
            Value::Object(upgrade)
        }
        _ => return, // tcp & friends: no transport section
    };

    outbound.insert("transport".into(), transport);
}

/// Some generators emit query params after `&` without a leading `?`
/// (e.g. `socks5://host:port&sni=...`); normalize that for the URL parser.
fn normalize_query(raw: &str) -> String {
    if raw.contains("://") && raw.split_once('#').is_none_or(|(before, _)| before.contains('?')) {
        return raw.to_string();
    }
    let (scheme_rest, fragment) = match raw.split_once('#') {
        Some((rest, fragment)) => (rest, Some(fragment)),
        None => (raw, None),
    };
    if !scheme_rest.contains('?') {
        if let Some(position) = scheme_rest.find('&') {
            let mut normalized = String::with_capacity(raw.len());
            normalized.push_str(&scheme_rest[..position]);
            normalized.push('?');
            normalized.push_str(&scheme_rest[position + 1..]);
            if let Some(fragment) = fragment {
                normalized.push('#');
                normalized.push_str(fragment);
            }
            return normalized;
        }
    }
    raw.to_string()
}

fn split_host_port(input: &str) -> Option<(String, u16)> {
    if let Some(rest) = input.strip_prefix('[') {
        let (host, port) = rest.split_once("]:")?;
        return Some((host.to_string(), port.parse().ok()?));
    }
    let (host, port) = input.rsplit_once(':')?;
    Some((host.to_string(), port.parse().ok()?))
}

fn decode_fragment(fragment: &str) -> String {
    percent_decode(fragment)
}

fn percent_decode(input: &str) -> String {
    percent_decode_str(input).decode_utf8_lossy().into_owned()
}

fn decode_base64(input: &str) -> Option<Vec<u8>> {
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

fn decode_base64_to_string(input: &str) -> Option<String> {
    decode_base64(input).and_then(|bytes| String::from_utf8(bytes).ok())
}

fn non_empty_lines(content: &str) -> usize {
    content.lines().filter(|line| !line.trim().is_empty()).count()
}

fn non_empty(value: Option<&String>) -> Option<&String> {
    value.filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opt-in check against a real subscription file:
    /// `MEGATHRONE_REAL_SUB=path/to/sub.txt cargo test --lib real_world_file`
    #[test]
    fn real_world_file() {
        let path = match std::env::var("MEGATHRONE_REAL_SUB") {
            Ok(path) => path,
            Err(_) => return,
        };
        let content = std::fs::read_to_string(&path).expect("read subscription file");
        let parsed = parse_subscription(&content);
        println!(
            "endpoints: {}, skipped: {}",
            parsed.endpoints.len(),
            parsed.skipped
        );
        assert!(parsed.endpoints.len() > 1000, "expected a large file to mostly parse");
        for endpoint in &parsed.endpoints {
            assert!(!endpoint.tag.is_empty());
            assert!(!endpoint.server.is_empty());
            assert!(endpoint.server_port > 0);
            serde_json::from_str::<Value>(&endpoint.outbound_json).expect("valid outbound JSON");
        }
    }

    fn endpoint_json(raw: &str) -> Value {
        let parsed = parse_link(raw).expect("link should parse");
        serde_json::from_str(&parsed.outbound_json).expect("outbound should be valid JSON")
    }

    #[test]
    fn parses_vless_ws_without_tls() {
        let raw = "vless://47fcef29-ab4e-4aa6-932b-d95a18f28a4e@104.18.47.113:80?security=none&type=ws&path=%2F%3Fed%3D2560&host=example.dev&sni=example.dev#%5BOpenRay%5D%20CA-1";
        let outbound = endpoint_json(raw);
        assert_eq!(outbound["type"], "vless");
        assert_eq!(outbound["uuid"], "47fcef29-ab4e-4aa6-932b-d95a18f28a4e");
        assert_eq!(outbound["server"], "104.18.47.113");
        assert_eq!(outbound["server_port"], 80);
        assert!(outbound.get("tls").is_none());
        assert_eq!(outbound["transport"]["type"], "ws");
        assert_eq!(outbound["transport"]["headers"]["Host"], "example.dev");
        let parsed = parse_link(raw).unwrap();
        assert_eq!(parsed.tag, "[OpenRay] CA-1");
    }

    #[test]
    fn parses_vless_reality() {
        let raw = "vless://uuid-1@1.2.3.4:443?security=reality&sni=www.apple.com&fp=chrome&pbk=PUBKEY123&sid=ab12&flow=xtls-rprx-vision&type=tcp#REALITY";
        let outbound = endpoint_json(raw);
        assert_eq!(outbound["tls"]["reality"]["public_key"], "PUBKEY123");
        assert_eq!(outbound["tls"]["reality"]["short_id"], "ab12");
        assert_eq!(outbound["tls"]["utls"]["fingerprint"], "chrome");
        assert_eq!(outbound["flow"], "xtls-rprx-vision");
        assert!(outbound.get("transport").is_none());
    }

    #[test]
    fn sanitizes_unknown_utls_fingerprints() {
        // plain tls: junk fingerprint is dropped, no utls block at all
        let outbound = endpoint_json(
            "vless://uuid-1@1.2.3.4:443?security=tls&sni=example.com&fp=unsafe&type=tcp#BadFP",
        );
        assert!(outbound["tls"].get("utls").is_none(), "junk fingerprint must be dropped");

        // case is normalized to what sing-box expects
        let outbound = endpoint_json(
            "vless://uuid-2@1.2.3.5:443?security=tls&sni=example.com&fp=CHROME&type=tcp#UpperFP",
        );
        assert_eq!(outbound["tls"]["utls"]["fingerprint"], "chrome");

        // reality needs a fingerprint: junk falls back to chrome
        let outbound = endpoint_json(
            "vless://uuid-3@1.2.3.6:443?security=reality&sni=www.apple.com&fp=unsafe&pbk=PUBKEY123&sid=ab12&type=tcp#RealityBadFP",
        );
        assert_eq!(outbound["tls"]["utls"]["fingerprint"], "chrome");
    }

    #[test]
    fn parses_vmess_base64() {
        let payload = serde_json::json!({
            "v": "2",
            "ps": "TEST VMESS",
            "add": "104.17.77.77",
            "port": "8443",
            "id": "60441548-b68e-43b2-8191-e3b884be4b3c",
            "aid": "0",
            "net": "ws",
            "path": "/lMnzZUN4/",
            "host": "v2ray1.dozapp.xyz",
            "tls": "tls"
        });
        let encoded = STANDARD.encode(payload.to_string());
        let raw = format!("vmess://{encoded}");
        let outbound = endpoint_json(&raw);
        assert_eq!(outbound["type"], "vmess");
        assert_eq!(outbound["server"], "104.17.77.77");
        assert_eq!(outbound["server_port"], 8443);
        assert_eq!(outbound["security"], "auto");
        assert_eq!(outbound["tls"]["enabled"], true);
        assert_eq!(outbound["transport"]["type"], "ws");
        assert_eq!(parse_link(&raw).unwrap().tag, "TEST VMESS");
    }

    #[test]
    fn parses_shadowsocks_base64_userinfo() {
        let raw = "ss://Y2hhY2hhMjAtaWV0Zi1wb2x5MTMwNTowZUhaTFVrTXc5UXB3N09hZGNab3QzcUp4UFp1R2Q4UQ==@de2.example.ir:43277?type=tcp#%F0%9F%8C%90%20Server%2040";
        let outbound = endpoint_json(raw);
        assert_eq!(outbound["type"], "shadowsocks");
        assert_eq!(outbound["method"], "chacha20-ietf-poly1305");
        assert_eq!(outbound["password"], "0eHZLUkMw9Qpw7OadcZot3qJxPZuGd8Q");
        assert_eq!(outbound["server"], "de2.example.ir");
        assert_eq!(outbound["server_port"], 43277);
        assert_eq!(parse_link(raw).unwrap().tag, "\u{1f310} Server 40");
    }

    #[test]
    fn parses_shadowsocks_plain_userinfo() {
        let raw = "ss://aes-256-gcm:azfowm5k13jgzld9@phooenixstore00.arh181.ir:59703#Plain%20SS";
        let outbound = endpoint_json(raw);
        assert_eq!(outbound["method"], "aes-256-gcm");
        assert_eq!(outbound["password"], "azfowm5k13jgzld9");
        assert_eq!(outbound["server_port"], 59703);
    }

    #[test]
    fn parses_trojan_ws_tls() {
        let raw = "trojan://SecretPass@188.114.97.8:443?path=%2Ftr%2FSniOCm&security=tls&alpn=http%2F1.1&host=worker.example.dev&fp=chrome&type=ws&sni=worker.example.dev#%5BOpenRay%5D%20CA-28994";
        let outbound = endpoint_json(raw);
        assert_eq!(outbound["type"], "trojan");
        assert_eq!(outbound["password"], "SecretPass");
        assert_eq!(outbound["tls"]["server_name"], "worker.example.dev");
        assert_eq!(outbound["tls"]["alpn"][0], "http/1.1");
        assert_eq!(outbound["transport"]["type"], "ws");
        assert_eq!(parse_link(raw).unwrap().tag, "[OpenRay] CA-28994");
    }

    #[test]
    fn parses_hysteria2_with_obfs() {
        let raw = "hy2://76ivwa51ce2gauym@giftcard.example.com:52004/?sni=giftcard.example.com&obfs=salamander&obfs-password=fuw2k1ddrouwxr3u#EPODONIOS";
        let outbound = endpoint_json(raw);
        assert_eq!(outbound["type"], "hysteria2");
        assert_eq!(outbound["password"], "76ivwa51ce2gauym");
        assert_eq!(outbound["obfs"]["type"], "salamander");
        assert_eq!(outbound["obfs"]["password"], "fuw2k1ddrouwxr3u");
        assert_eq!(outbound["tls"]["server_name"], "giftcard.example.com");
        assert_eq!(parse_link(raw).unwrap().tag, "EPODONIOS");
    }

    #[test]
    fn parses_tuic_v5() {
        let raw = "tuic://2d1ad594-80a4-4bfb-87a6-038e39701f51:pass123@sg2.example.org:33738?alpn=h3&congestion_control=bbr&sni=sg2.example.org&udp_relay_mode=quic#SG";
        let outbound = endpoint_json(raw);
        assert_eq!(outbound["type"], "tuic");
        assert_eq!(outbound["uuid"], "2d1ad594-80a4-4bfb-87a6-038e39701f51");
        assert_eq!(outbound["password"], "pass123");
        assert_eq!(outbound["congestion_control"], "bbr");
        assert_eq!(outbound["udp_relay_mode"], "quic");
    }

    #[test]
    fn parses_socks5_with_ampersand_query() {
        let raw = "socks5://user:pass@128.140.46.169:13482&host=socks5.example.workers.dev&sni=socks5.example.workers.dev#By%20EbraSha";
        let outbound = endpoint_json(raw);
        assert_eq!(outbound["type"], "socks");
        assert_eq!(outbound["version"], "5");
        assert_eq!(outbound["server"], "128.140.46.169");
        assert_eq!(outbound["server_port"], 13482);
        assert_eq!(outbound["username"], "user");
        assert_eq!(outbound["password"], "pass");
    }

    #[test]
    fn skips_ssr_and_comments() {
        let content = "ssr://somethingopaque\nvless://47fcef29-ab4e-4aa6-932b-d95a18f28a4e@1.2.3.4:80?security=none#Test\n# comment line";
        let parsed = parse_subscription(content);
        assert_eq!(parsed.endpoints.len(), 1);
        assert_eq!(parsed.skipped, 1);
        assert_eq!(parsed.endpoints[0].tag, "Test");
    }

    #[test]
    fn parses_base64_subscription_payload() {
        let links = "vless://47fcef29-ab4e-4aa6-932b-d95a18f28a4e@1.2.3.4:443?security=tls#One\ntrojan://pw@5.6.7.8:443?security=tls#Two";
        let encoded = STANDARD.encode(links);
        let parsed = parse_subscription(&encoded);
        assert_eq!(parsed.endpoints.len(), 2);
        assert_eq!(parsed.endpoints[0].tag, "One");
    }

    #[test]
    fn parses_native_singbox_config() {
        let config = serde_json::json!({
            "outbounds": [
                { "type": "selector", "tag": "default", "outbounds": ["auto", "proxy"] },
                { "type": "vless", "tag": "proxy", "server": "1.2.3.4", "server_port": 443, "uuid": "u" }
            ]
        });
        let parsed = parse_subscription(&config.to_string());
        assert_eq!(parsed.endpoints.len(), 1);
        assert_eq!(parsed.endpoints[0].tag, "proxy");
        assert_eq!(parsed.skipped, 1);
    }

    #[test]
    fn deduplicates_tags() {
        let content = "trojan://pw@1.1.1.1:443?security=tls#Same\ntrojan://pw@2.2.2.2:443?security=tls#Same\ntrojan://pw@3.3.3.3:443?security=tls#Same";
        let parsed = parse_subscription(content);
        let tags: Vec<&str> = parsed.endpoints.iter().map(|e| e.tag.as_str()).collect();
        assert_eq!(tags, vec!["Same", "Same #2", "Same #3"]);
    }
}
