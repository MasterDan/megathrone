use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::Value;

use super::links::parse_link;
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
