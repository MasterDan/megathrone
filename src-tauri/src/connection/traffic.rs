//! The live-session charts: the ~1 Hz clash `/connections` poller folding
//! per-outbound byte deltas into `traffic-stats` events, and the
//! session-scoped per-URL request table served by `connection_url_stats`.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use super::state::{DIRECT_TAG, DPI_TAG, PROXY_TAG};

/// Emitted ~once a second while the proxy runs: cumulative bytes that went
/// through each of the session's outbounds (up = client → internet).
pub const TRAFFIC_STATS_EVENT: &str = "traffic-stats";

/// How often the clash API is polled for per-connection counters.
const TRAFFIC_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficStats {
    pub proxy_up: u64,
    pub proxy_down: u64,
    pub dpi_up: u64,
    pub dpi_down: u64,
    pub direct_up: u64,
    pub direct_down: u64,
}

/// One row of the per-URL session summary: how many connections (requests)
/// one process opened to one host through one tunnel, split into successful
/// (closed having carried response bytes back) and failed (closed mute —
/// connected, but not a single byte came back). `process` is the binary
/// name sing-box resolved for the connection (empty when unknown — another
/// user's socket the platform refuses to attribute). Session-scoped — a
/// fresh connect clears the table, a disconnect leaves the last snapshot
/// in place so the page keeps showing what the session did.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UrlStatEntry {
    pub process: String,
    pub host: String,
    /// "proxy" | "dpi" | "direct"
    pub tunnel: String,
    /// every connection seen this session — the still-open ones included
    pub requests: u64,
    /// closed with response bytes
    pub ok: u64,
    /// closed without a single response byte
    pub failed: u64,
}

/// The per-URL table's key: who opened the connection, where it went and
/// through which tunnel.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct UrlKey {
    process: String,
    host: String,
    tunnel: String,
}

/// Running counters behind one (host, tunnel) row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct UrlCounters {
    requests: u64,
    ok: u64,
    failed: u64,
}

/// The current session's per-URL summary, republished by the traffic poller
/// on every tick; readable from the UI at any time.
static URL_STATS: LazyLock<Mutex<Vec<UrlStatEntry>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Generation key for the poller task: connect bumps it and spawns a poller
/// holding the new value; every teardown path bumps it again so a poller of
/// a dead session exits on its next tick.
static TRAFFIC_POLL_GEN: AtomicU64 = AtomicU64::new(0);

/// The current (or just-ended) session's per-URL summary: which host was
/// reached through which tunnel, and how many requests went there —
/// collected by the traffic poller off the clash `/connections` snapshots.
#[tauri::command]
pub fn connection_url_stats() -> Result<Vec<UrlStatEntry>, String> {
    URL_STATS.lock().map(|stats| stats.clone()).map_err(|e| e.to_string())
}

pub(super) fn start_traffic_poller(app: &AppHandle, api_port: u16) {
    // a fresh session counts from zero (a disconnect deliberately keeps the
    // last table for post-session viewing)
    if let Ok(mut stats) = URL_STATS.lock() {
        stats.clear();
    }
    let gen = TRAFFIC_POLL_GEN.fetch_add(1, Ordering::Relaxed) + 1;
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || traffic_poll_loop(&app, api_port, gen));
}

pub(super) fn stop_traffic_poller() {
    TRAFFIC_POLL_GEN.fetch_add(1, Ordering::Relaxed);
}

/// Polls the clash `/connections` endpoint of the running sing-box: every
/// open connection carries its `chains` (the outbound it rides — `mt-proxy`
/// / `mt-dpi` / `mt-direct` in our config) and cumulative byte counters, so
/// per-connection deltas accumulate into per-outbound totals. Blocking —
/// runs on a `spawn_blocking` thread; exits once its generation is
/// superseded (disconnect, unexpected exit, a fresh connect).
fn traffic_poll_loop(app: &AppHandle, api_port: u16, gen: u64) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(1)))
        .build()
        .into();
    let url = format!("http://127.0.0.1:{api_port}/connections");
    let mut seen: HashMap<String, (u64, u64)> = HashMap::new();
    let mut totals = TrafficStats::default();
    let mut urls: HashMap<UrlKey, UrlCounters> = HashMap::new();
    // open connections' (process, host, tunnel) — kept until the id leaves
    // the snapshot, so its outcome can be finalized then
    let mut meta: HashMap<String, UrlKey> = HashMap::new();
    loop {
        std::thread::sleep(TRAFFIC_POLL_INTERVAL);
        if TRAFFIC_POLL_GEN.load(Ordering::Relaxed) != gen {
            return;
        }
        let body = agent.get(&url).call().ok().and_then(|mut response| {
            response.body_mut().with_config().limit(4 << 20).read_to_string().ok()
        });
        let Some(body) = body else { continue };
        let Ok(payload) = serde_json::from_str::<Value>(&body) else {
            continue;
        };
        accumulate_connections(&payload, &mut seen, &mut totals, &mut urls, &mut meta);
        publish_url_stats(&urls);
        let _ = app.emit(TRAFFIC_STATS_EVENT, totals);
    }
}

/// Republishes the per-URL counters as a list sorted by requests (ties by
/// process, then host, then tunnel, so the order is stable between ticks).
fn publish_url_stats(urls: &HashMap<UrlKey, UrlCounters>) {
    let mut entries: Vec<UrlStatEntry> = urls
        .iter()
        .map(|(key, counters)| UrlStatEntry {
            process: key.process.clone(),
            host: key.host.clone(),
            tunnel: key.tunnel.clone(),
            requests: counters.requests,
            ok: counters.ok,
            failed: counters.failed,
        })
        .collect();
    entries.sort_by(|a, b| {
        b.requests
            .cmp(&a.requests)
            .then_with(|| a.process.cmp(&b.process))
            .then_with(|| a.host.cmp(&b.host))
            .then_with(|| a.tunnel.cmp(&b.tunnel))
    });
    if let Ok(mut stats) = URL_STATS.lock() {
        *stats = entries;
    }
}

/// Folds one `/connections` payload into the totals: `seen` holds the last
/// snapshot's counters per connection id and is updated in place (closed
/// connections drop out — their bytes were counted while alive).
/// Connections riding none of our outbounds are ignored. Connection ids
/// absent from `seen` are new — each one counts as one request by its
/// process to its host through its tunnel (`meta` remembers the id →
/// attribution key). A connection leaving the snapshot is finalized then:
/// response bytes seen (`download > 0` at its last sighting) make it a
/// successful request, closing mute makes it a failed one — still-open
/// connections stay counted as requests but neither ok nor failed.
fn accumulate_connections(
    payload: &Value,
    seen: &mut HashMap<String, (u64, u64)>,
    totals: &mut TrafficStats,
    urls: &mut HashMap<UrlKey, UrlCounters>,
    meta: &mut HashMap<String, UrlKey>,
) {
    let Some(connections) = payload.get("connections").and_then(Value::as_array) else {
        return;
    };
    let mut present: HashSet<&str> = HashSet::new();
    for connection in connections {
        let Some(id) = connection.get("id").and_then(Value::as_str) else { continue };
        present.insert(id);
        let upload = connection.get("upload").and_then(Value::as_u64).unwrap_or(0);
        let download = connection.get("download").and_then(Value::as_u64).unwrap_or(0);
        let Some(chains) = connection.get("chains").and_then(Value::as_array) else {
            continue;
        };
        let (up, down) = seen.get(id).copied().unwrap_or((0, 0));
        let delta_up = upload.saturating_sub(up);
        let delta_down = download.saturating_sub(down);
        match connection_tunnel(chains) {
            Some("proxy") => {
                totals.proxy_up += delta_up;
                totals.proxy_down += delta_down;
            }
            Some("dpi") => {
                totals.dpi_up += delta_up;
                totals.dpi_down += delta_down;
            }
            Some("direct") => {
                totals.direct_up += delta_up;
                totals.direct_down += delta_down;
            }
            _ => {}
        }
        if !seen.contains_key(id) {
            if let (Some(tunnel), Some(host)) = (connection_tunnel(chains), connection_host(connection)) {
                let key = UrlKey {
                    process: connection_process(connection),
                    host,
                    tunnel: tunnel.to_string(),
                };
                urls.entry(key.clone()).or_default().requests += 1;
                meta.insert(id.to_string(), key);
            }
        }
        seen.insert(id.to_string(), (upload, download));
    }
    // connections that left the snapshot since the last tick are done —
    // finalize each one's outcome by what it managed to carry
    let closed: Vec<String> = seen
        .keys()
        .filter(|id| !present.contains(id.as_str()))
        .cloned()
        .collect();
    for id in closed {
        let Some(key) = meta.remove(&id) else {
            seen.remove(&id); // never attributed (foreign chain / no host)
            continue;
        };
        let (_, download) = seen.remove(&id).unwrap_or((0, 0));
        let counters = urls.entry(key).or_default();
        if download > 0 {
            counters.ok += 1;
        } else {
            counters.failed += 1;
        }
    }
}

/// The session tunnel a connection rode — `"proxy"`, `"dpi"` or `"direct"`,
/// read off its clash `chains` (`mt-proxy` / `mt-dpi` / `mt-direct` in our
/// config). `None` for anything not one of our outbounds.
fn connection_tunnel(chains: &[Value]) -> Option<&'static str> {
    if chains.iter().any(|tag| tag.as_str() == Some(PROXY_TAG)) {
        Some("proxy")
    } else if chains.iter().any(|tag| tag.as_str() == Some(DPI_TAG)) {
        Some("dpi")
    } else if chains.iter().any(|tag| tag.as_str() == Some(DIRECT_TAG)) {
        Some("direct")
    } else {
        None
    }
}

/// The host a connection went to: the clash metadata's `host` (sniffed or
/// dialed domain), or the raw `destinationIP:destinationPort` when no
/// hostname is known. `None` when neither is present.
fn connection_host(connection: &Value) -> Option<String> {
    let metadata = connection.get("metadata")?;
    let host = metadata.get("host").and_then(Value::as_str).unwrap_or("");
    if !host.is_empty() {
        return Some(host.to_string());
    }
    let ip = metadata.get("destinationIP").and_then(Value::as_str).unwrap_or("");
    if ip.is_empty() {
        return None;
    }
    let port = metadata
        .get("destinationPort")
        .and_then(|value| value.as_str().map(str::to_string).or_else(|| value.as_u64().map(|n| n.to_string())))
        .unwrap_or_default();
    Some(if port.is_empty() { ip.to_string() } else { format!("{ip}:{port}") })
}

/// The process that opened the connection: the clash metadata's
/// `processPath` (populated only because the run config sets
/// `find_process` — without it sing-box never resolves the field), cut to
/// its basename — the binary name users recognize. On Android the field
/// carries package names, which have no separators and pass through
/// unchanged. Empty when the process is unknown.
fn connection_process(connection: &Value) -> String {
    let path = connection
        .get("metadata")
        .and_then(|metadata| metadata.get("processPath"))
        .and_then(Value::as_str)
        .unwrap_or("");
    path.rsplit(['/', '\\']).next().unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Consecutive `/connections` snapshots accumulate per-outbound deltas:
    /// counters grow, connections close (their last bytes were already
    /// counted), new ones appear, unknown chains are ignored.
    #[test]
    fn traffic_totals_accumulate_deltas_per_outbound() {
        let mut seen = HashMap::new();
        let mut totals = TrafficStats::default();
        let mut urls = HashMap::new();
        let mut meta = HashMap::new();

        let first = json!({
            "connections": [
                { "id": "a", "upload": 100, "download": 500, "chains": ["mt-proxy"] },
                { "id": "b", "upload": 10, "download": 20, "chains": ["mt-dpi"] },
                { "id": "c", "upload": 7, "download": 9, "chains": ["mt-direct"] },
                { "id": "d", "upload": 1, "download": 1, "chains": ["foreign"] }
            ]
        });
        accumulate_connections(&first, &mut seen, &mut totals, &mut urls, &mut meta);
        assert_eq!(
            totals,
            TrafficStats {
                proxy_up: 100, proxy_down: 500,
                dpi_up: 10, dpi_down: 20,
                direct_up: 7, direct_down: 9,
            }
        );

        // "a" grows, "b" stalls, "c" closed (final bytes counted last tick),
        // "e" is new, "d" (unknown chain) is gone
        let second = json!({
            "connections": [
                { "id": "a", "upload": 300, "download": 900, "chains": ["mt-proxy"] },
                { "id": "b", "upload": 10, "download": 40, "chains": ["mt-dpi"] },
                { "id": "e", "upload": 5, "download": 5, "chains": ["mt-direct"] }
            ]
        });
        accumulate_connections(&second, &mut seen, &mut totals, &mut urls, &mut meta);
        assert_eq!(
            totals,
            TrafficStats {
                proxy_up: 300, proxy_down: 900,
                dpi_up: 10, dpi_down: 40,
                direct_up: 12, direct_down: 14,
            }
        );
        assert_eq!(seen.len(), 3, "closed and foreign connections drop out");

        // a payload without connections leaves everything untouched
        accumulate_connections(&json!({}), &mut seen, &mut totals, &mut urls, &mut meta);
        assert_eq!(seen.len(), 3);
        assert_eq!(totals.proxy_up, 300);
    }

    /// Each connection id unseen in the previous snapshot is one request:
    /// counted per (process, host, tunnel), surviving ids never
    /// double-count, processes fall back to empty when sing-box could not
    /// resolve one, hosts fall back to the raw IP:port when no hostname is
    /// known — and a connection leaving the snapshot finalizes its outcome
    /// by whether it ever carried response bytes back.
    #[test]
    fn url_stats_count_new_connections_per_process_host_and_tunnel() {
        fn counters(
            urls: &HashMap<UrlKey, UrlCounters>,
            process: &str,
            host: &str,
            tunnel: &str,
        ) -> UrlCounters {
            *urls
                .get(&UrlKey {
                    process: process.to_string(),
                    host: host.to_string(),
                    tunnel: tunnel.to_string(),
                })
                .unwrap()
        }

        let mut seen = HashMap::new();
        let mut totals = TrafficStats::default();
        let mut urls = HashMap::new();
        let mut meta = HashMap::new();

        let first = json!({
            "connections": [
                {
                    "id": "a", "upload": 1, "download": 1, "chains": ["mt-proxy"],
                    "metadata": { "host": "example.com", "destinationIP": "1.2.3.4", "destinationPort": "443", "processPath": "/usr/bin/curl" }
                },
                {
                    "id": "b", "upload": 1, "download": 1, "chains": ["mt-dpi"],
                    "metadata": { "host": "example.com", "destinationIP": "1.2.3.4", "destinationPort": "443", "processPath": "/usr/bin/curl" }
                },
                {
                    "id": "c", "upload": 1, "download": 1, "chains": ["mt-direct"],
                    "metadata": { "host": "", "destinationIP": "5.6.7.8", "destinationPort": "853", "processPath": "/usr/local/bin/python3.12" }
                },
                {
                    "id": "d", "upload": 1, "download": 1, "chains": ["mt-proxy"],
                    "metadata": { "host": "", "destinationIP": "", "processPath": "/usr/bin/wget" }
                }
            ]
        });
        accumulate_connections(&first, &mut seen, &mut totals, &mut urls, &mut meta);
        assert_eq!(counters(&urls, "curl", "example.com", "proxy").requests, 1);
        assert_eq!(counters(&urls, "curl", "example.com", "dpi").requests, 1);
        assert_eq!(counters(&urls, "python3.12", "5.6.7.8:853", "direct").requests, 1);
        assert_eq!(urls.len(), 3, "no host to attribute — not counted");
        assert_eq!(
            counters(&urls, "curl", "example.com", "proxy"),
            UrlCounters { requests: 1, ok: 0, failed: 0 },
            "still-open connections are neither ok nor failed yet"
        );

        // "a" rides on, "b" and "c" closed (both had response bytes → ok),
        // "d" never had a host so its closing counts for nothing; "e" and
        // "f" are new — "e" is another process hitting the same host
        // (its own row), "f" carries no process info and proves numeric
        // ports count too
        let second = json!({
            "connections": [
                {
                    "id": "a", "upload": 9, "download": 9, "chains": ["mt-proxy"],
                    "metadata": { "host": "example.com", "processPath": "/usr/bin/curl" }
                },
                {
                    "id": "e", "upload": 5, "download": 0, "chains": ["mt-proxy"],
                    "metadata": { "host": "example.com", "processPath": "/Applications/Browser.app/Contents/MacOS/Browser" }
                },
                {
                    "id": "f", "upload": 1, "download": 1, "chains": ["mt-direct"],
                    "metadata": { "destinationIP": "5.6.7.8", "destinationPort": 853 }
                }
            ]
        });
        accumulate_connections(&second, &mut seen, &mut totals, &mut urls, &mut meta);
        assert_eq!(counters(&urls, "curl", "example.com", "proxy").requests, 1);
        assert_eq!(counters(&urls, "Browser", "example.com", "proxy").requests, 1);
        assert_eq!(counters(&urls, "", "5.6.7.8:853", "direct").requests, 1);
        assert_eq!(
            counters(&urls, "curl", "example.com", "dpi"),
            UrlCounters { requests: 1, ok: 1, failed: 0 }
        );
        assert_eq!(
            counters(&urls, "python3.12", "5.6.7.8:853", "direct"),
            UrlCounters { requests: 1, ok: 1, failed: 0 }
        );
        assert_eq!(seen.len(), 3);

        // "e" closed mute (sent 5 bytes, nothing came back → failed), "f"
        // closed with response bytes (→ its ok); "a" still open
        let third = json!({
            "connections": [
                {
                    "id": "a", "upload": 12, "download": 10, "chains": ["mt-proxy"],
                    "metadata": { "host": "example.com", "processPath": "/usr/bin/curl" }
                }
            ]
        });
        accumulate_connections(&third, &mut seen, &mut totals, &mut urls, &mut meta);
        assert_eq!(
            counters(&urls, "curl", "example.com", "proxy"),
            UrlCounters { requests: 1, ok: 0, failed: 0 },
            "the open \"a\" is still neither — only \"e\" finalized, as failed"
        );
        assert_eq!(
            counters(&urls, "Browser", "example.com", "proxy"),
            UrlCounters { requests: 1, ok: 0, failed: 1 }
        );
        assert_eq!(
            counters(&urls, "", "5.6.7.8:853", "direct"),
            UrlCounters { requests: 1, ok: 1, failed: 0 }
        );
        assert_eq!(seen.len(), 1);
        assert_eq!(meta.len(), 1);
    }

    /// `processPath` becomes the binary's name: unix and windows separators
    /// both cut, bare names (Android packages) pass through, and anything
    /// missing stays empty.
    #[test]
    fn connection_process_takes_the_path_basename() {
        fn process(metadata: Value) -> String {
            connection_process(&json!({ "metadata": metadata }))
        }
        assert_eq!(process(json!({ "processPath": "/usr/bin/curl" })), "curl");
        assert_eq!(
            process(json!({ "processPath": "C:\\Program Files\\App\\app.exe" })),
            "app.exe"
        );
        assert_eq!(process(json!({ "processPath": "com.example.app" })), "com.example.app");
        assert_eq!(process(json!({ "processPath": "" })), "");
        assert_eq!(process(json!({})), "");
        assert_eq!(connection_process(&json!({})), "");
    }
}
