use std::collections::HashMap;

use serde_json::{json, Map, Value};

use super::utils::normalize_fingerprint;

/// `security=tls` (or `reality`) turns TLS on; reality carries pbk/sid.
pub(super) fn build_tls(params: &HashMap<String, String>, enabled_by_default: bool, outbound: &mut Map<String, Value>) {
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
pub(super) fn build_transport(params: &HashMap<String, String>, outbound: &mut Map<String, Value>) {
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
