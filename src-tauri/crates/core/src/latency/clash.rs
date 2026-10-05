use std::time::Duration;

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::Value;

use super::storage::{LatencyResult, UrlProbe};

/// The URL every proxy has to fetch during the test (Clash convention).
const TEST_URL: &str = "http://www.gstatic.com/generate_204";
/// Per-endpoint budget for the whole proxy roundtrip (connect + TLS + HTTP).
pub(super) const TEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Tests a batch in parallel on scoped threads (the caller guarantees the
/// batch is small enough).
pub(super) fn delay_test_chunk(base_url: &str, targets: &[(i64, String)], timeout: Duration) -> Vec<LatencyResult> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout + Duration::from_secs(2)))
        .build()
        .into();
    std::thread::scope(|scope| {
        let handles: Vec<_> = targets
            .iter()
            .map(|(id, tag)| {
                let agent = &agent;
                scope.spawn(move || delay_test_one(agent, base_url, tag, timeout, *id))
            })
            .collect();
        handles
            .into_iter()
            .zip(targets)
            .map(|(handle, (id, _))| {
                handle.join().unwrap_or_else(|_| LatencyResult::failed(*id))
            })
            .collect()
    })
}

/// The base availability probe: the built-in URL through one proxy.
fn delay_test_one(
    agent: &ureq::Agent,
    base_url: &str,
    tag: &str,
    timeout: Duration,
    id: i64,
) -> LatencyResult {
    let Some(delay) = proxy_url_delay(agent, base_url, tag, TEST_URL, timeout) else {
        return LatencyResult::failed(id);
    };
    LatencyResult { id, available: true, latency_ms: Some(delay), url_ok: 0, url_total: 0 }
}

/// One URL test through a proxy: `GET /proxies/{tag}/delay?url=…` makes
/// sing-box fetch `url` through that outbound and answer with the measured
/// delay in ms. `None` = not reachable. The URL is percent-encoded for the
/// query string (user URLs carry `&`, `?`, spaces, non-ASCII…).
fn proxy_url_delay(
    agent: &ureq::Agent,
    base_url: &str,
    tag: &str,
    url: &str,
    timeout: Duration,
) -> Option<i64> {
    let encoded = utf8_percent_encode(url, NON_ALPHANUMERIC);
    let request =
        format!("{base_url}/proxies/{tag}/delay?url={encoded}&timeout={}", timeout.as_millis());

    let Ok(mut response) = agent.get(&request).call() else {
        return None;
    };
    if response.status().as_u16() != 200 {
        return None;
    }
    let Ok(body) = response.body_mut().with_config().limit(4096).read_to_string() else {
        return None;
    };
    let Ok(value) = serde_json::from_str::<Value>(&body) else {
        return None;
    };
    match value.get("delay").and_then(Value::as_i64) {
        Some(delay) if delay >= 0 => Some(delay),
        _ => None,
    }
}

/// Tests (endpoint, custom URL) pairs in parallel on scoped threads.
pub fn url_test_chunk(
    base_url: &str,
    targets: &[(i64, String, i64, String)],
    timeout: Duration,
) -> Vec<UrlProbe> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout + Duration::from_secs(2)))
        .build()
        .into();
    std::thread::scope(|scope| {
        let handles: Vec<_> = targets
            .iter()
            .map(|(endpoint_id, tag, url_id, url)| {
                let agent = &agent;
                scope.spawn(move || {
                    (*endpoint_id, *url_id, proxy_url_delay(agent, base_url, tag, url, timeout))
                })
            })
            .collect();
        // a panicked worker yields an impossible id 0 sentinel, dropped by
        // the caller's ok-count lookup
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap_or((0, 0, None)))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use super::*;

    /// A minimal fake of the clash delay API: one request per connection,
    /// `Connection: close`, canned responses picked by the proxy tag in the
    /// request path.
    #[test]
    fn delay_test_chunk_parses_clash_api_responses() {
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
                    let (status, body) = if head.contains("/proxies/mt-good/") {
                        ("200 OK", r#"{"delay":42}"#)
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

            let results = delay_test_chunk(
                &base_url,
                &[(10, "mt-good".into()), (11, "mt-bad".into())],
                Duration::from_secs(2),
            );

            assert_eq!(results.len(), 2);
            let good = results.iter().find(|result| result.id == 10).expect("good result");
            assert!(good.available);
            assert_eq!(good.latency_ms, Some(42));
            assert_eq!((good.url_ok, good.url_total), (0, 0), "counts are filled by the scan loop");
            let bad = results.iter().find(|result| result.id == 11).expect("bad result");
            assert!(!bad.available);
            assert!(bad.latency_ms.is_none());
        });
    }
}
