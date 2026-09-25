use std::collections::HashMap;

use percent_encoding::percent_decode_str;
use serde_json::{json, Map, Value};
use url::Url;

use super::builders::{build_tls, build_transport};
use super::subscription::NewEndpoint;
use super::utils::decode_base64;

pub(super) fn parse_link(raw: &str) -> Option<NewEndpoint> {
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

fn decode_base64_to_string(input: &str) -> Option<String> {
    decode_base64(input).and_then(|bytes| String::from_utf8(bytes).ok())
}

fn non_empty(value: Option<&String>) -> Option<&String> {
    value.filter(|s| !s.is_empty())
}
