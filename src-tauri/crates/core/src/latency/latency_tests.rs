use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use rusqlite::Connection;
use serde_json::json;

use super::*;

pub(super) fn test_db() -> Connection {
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "megathrone-latency-{}-{id}.db",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    crate::db::open(&path, &crate::db::Migrations::embedded()).expect("test db should open")
}

#[test]
fn build_test_config_retags_and_wires_the_clash_api() {
    let first = json!({
        "type": "vless", "tag": "original #2", "server": "1.2.3.4", "server_port": 443
    });
    let second = json!({ "type": "socks", "server": "5.6.7.8", "server_port": 1080 });

    let config = build_test_config(&[first, second], 45678);

    let outbounds = config["outbounds"].as_array().expect("outbounds");
    assert_eq!(outbounds.len(), 3, "endpoint outbounds + the direct default");
    assert_eq!(outbounds[0]["tag"], "mt-0", "source tags are replaced");
    assert_eq!(outbounds[0]["type"], "vless");
    assert_eq!(outbounds[1]["tag"], "mt-1");
    assert_eq!(outbounds[2]["type"], "direct");
    assert_eq!(outbounds[2]["tag"], "mt-direct");
    assert_eq!(config["route"]["final"], "mt-direct");
    assert_eq!(
        config["experimental"]["clash_api"]["external_controller"],
        "127.0.0.1:45678"
    );
}

/// Same fake API shape, but the canned response is picked by the
/// *encoded* custom URL in the query string — proving user URLs with
/// `&`/`?` survive the trip to the clash API — plus the tag.
#[test]
fn url_test_chunk_encodes_urls_and_reports_per_url() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake api");
    let port = listener.local_addr().expect("addr").port();
    let base_url = format!("http://127.0.0.1:{port}");

    std::thread::scope(|scope| {
        scope.spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().expect("accept");
                let mut request = Vec::new();
                let mut buffer = [0u8; 1024];
                loop {
                    let read = stream.read(&mut buffer).expect("read request");
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let head = String::from_utf8_lossy(&request);
                let (status, body) = if head.contains("/proxies/mt-good/")
                    && head.contains("url=http%3A%2F%2Fhost%2Fcheck%3Fq%3D1%26z%3D2")
                {
                    ("200 OK", r#"{"delay":73}"#)
                } else {
                    ("408 Request Timeout", r#"{"message":"context deadline exceeded"}"#)
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("write response");
            }
        });

        let nasty = "http://host/check?q=1&z=2";
        let probes = url_test_chunk(
            &base_url,
            &[
                (7, "mt-good".into(), 10, nasty.into()),
                (7, "mt-bad".into(), 10, nasty.into()),
            ],
            Duration::from_secs(2),
        );

        assert_eq!(probes.len(), 2);
        assert_eq!(probes[0], (7, 10, Some(73)));
        assert_eq!(probes[1], (7, 10, None));
    });
}

#[test]
fn build_test_config_drops_unknown_fingerprints() {
    let poisoned = json!({
        "type": "vless", "server": "1.2.3.4", "server_port": 443,
        "tls": { "enabled": true, "utls": { "enabled": true, "fingerprint": "unsafe" } }
    });

    let config = build_test_config(&[poisoned], 45678);

    let utls = &config["outbounds"][0]["tls"]["utls"];
    assert!(utls.get("fingerprint").is_none(), "junk fingerprint must be dropped");
    assert_eq!(utls["enabled"], true, "uTLS stays enabled with the default");
}

#[test]
fn load_outbounds_rejects_unknown_profile() {
    let conn = test_db();
    let error = load_outbounds(&conn, 999).expect_err("unknown profile");
    assert!(error.contains("not found"));
}
