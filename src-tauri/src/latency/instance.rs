use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tauri::AppHandle;

use super::storage::OutboundRow;
use crate::parser;

/// How long the throwaway sing-box instance may take to expose its API.
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);

/// Internal tags in the generated config: ASCII-only, unique, no encoding
/// issues in clash API paths.
const TAG_PREFIX: &str = "mt-";
const DIRECT_TAG: &str = "mt-direct";

pub fn sidecar() -> Result<crate::process::Command, String> {
    crate::process::sidecar("sing-box")
        .map_err(|error| format!("failed to resolve sing-box sidecar: {error}"))
}

/// Kills the throwaway sing-box instance and removes its config when the scan
/// ends for any reason (including early returns and panics).
pub struct SingBoxGuard {
    pub(crate) child: Option<crate::process::CommandChild>,
    pub(crate) config_path: PathBuf,
    /// set when the instance failed to start — keep the config for debugging
    pub(crate) keep_config: bool,
}

impl Drop for SingBoxGuard {
    fn drop(&mut self) {
        if let Some(child) = self.child.take() {
            let _ = child.kill();
        }
        if !self.keep_config && !self.config_path.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.config_path);
        }
    }
}

pub(super) fn tag_for(index: usize) -> String {
    format!("{TAG_PREFIX}{index}")
}

/// Drops uTLS fingerprints sing-box does not know — rows stored before the
/// parser sanitized them (e.g. `fp=unsafe`) would otherwise make sing-box
/// reject the whole config. uTLS stays enabled with its default.
pub fn sanitize_outbound(outbound: &mut Value) {
    let Some(tls) = outbound.get_mut("tls").and_then(Value::as_object_mut) else {
        return;
    };
    let Some(utls) = tls.get_mut("utls").and_then(Value::as_object_mut) else {
        return;
    };
    let unknown = utls
        .get("fingerprint")
        .and_then(Value::as_str)
        .is_some_and(|fingerprint| {
            !parser::UTLS_FINGERPRINTS.contains(&fingerprint.to_ascii_lowercase().as_str())
        });
    if unknown {
        utls.remove("fingerprint");
    }
}

/// Builds the test config: every endpoint as an outbound with a synthetic
/// tag (uniqueness and ASCII guaranteed regardless of the source tags), a
/// direct outbound as the route default, and the clash API on localhost.
pub fn build_test_config(outbounds: &[Value], api_port: u16) -> Value {
    let mut tagged: Vec<Value> = outbounds
        .iter()
        .enumerate()
        .map(|(index, outbound)| {
            let mut outbound = outbound.clone();
            sanitize_outbound(&mut outbound);
            outbound["tag"] = Value::String(tag_for(index));
            outbound
        })
        .collect();
    tagged.push(json!({ "type": "direct", "tag": DIRECT_TAG }));

    json!({
        "log": { "disabled": true },
        "outbounds": tagged,
        "route": { "final": DIRECT_TAG },
        "experimental": {
            "clash_api": { "external_controller": format!("127.0.0.1:{api_port}") }
        }
    })
}

pub fn write_config_file(profile_id: i64, api_port: u16, config: &Value) -> Result<PathBuf, String> {
    let path = std::env::temp_dir().join(format!("megathrone-urltest-{profile_id}-{api_port}.json"));
    let content =
        serde_json::to_string_pretty(config).map_err(|e| format!("failed to serialize config: {e}"))?;
    std::fs::write(&path, content).map_err(|e| format!("failed to write {}: {e}", path.display()))?;
    Ok(path)
}

pub fn free_local_port() -> Result<u16, String> {
    TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("cannot pick a local port: {e}"))?
        .local_addr()
        .map_err(|e| format!("cannot pick a local port: {e}"))
        .map(|address| address.port())
}

/// `sing-box check` on a throwaway config built from `values`; must run on a
/// blocking thread (it drives an async sidecar call via `block_on`).
pub(super) fn run_sing_box_check(api_port: u16, values: &[Value]) -> Result<bool, String> {
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("megathrone-urltest-check-{unique}.json"));
    let content = serde_json::to_string_pretty(&build_test_config(values, api_port))
        .map_err(|e| format!("failed to serialize config: {e}"))?;
    std::fs::write(&path, content).map_err(|e| format!("failed to write {}: {e}", path.display()))?;

    let check = sidecar()?;
    let output = tauri::async_runtime::block_on(check.args(["check", "-c", &path.to_string_lossy()]).output())
        .map_err(|error| format!("failed to run sing-box check: {error}"))?;
    let _ = std::fs::remove_file(&path);
    Ok(output.status.success())
}

/// Isolates outbounds the current sing-box refuses to load (legacy rows,
/// foreign generators) by bisection: `check` validates a whole config
/// cheaply, so a failing half is split again until the individual offenders
/// are found. They end up in the "broken" list and are reported as
/// unavailable instead of killing the whole scan.
pub(super) fn isolate_bad_outbounds(
    rows: &[OutboundRow],
    check: &impl Fn(&[Value]) -> Result<bool, String>,
) -> Result<(Vec<OutboundRow>, Vec<i64>), String> {
    let values: Vec<Value> = rows.iter().map(|(_, outbound)| outbound.clone()).collect();
    if check(&values)? {
        return Ok((rows.to_vec(), Vec::new()));
    }
    if rows.len() == 1 {
        return Ok((Vec::new(), vec![rows[0].0]));
    }
    let (left, right) = rows.split_at(rows.len() / 2);
    let (mut good, mut broken) = isolate_bad_outbounds(left, check)?;
    let (good_right, broken_right) = isolate_bad_outbounds(right, check)?;
    good.extend(good_right);
    broken.extend(broken_right);
    Ok((good, broken))
}

/// `isolate_bad_outbounds` driven by the real sidecar — the scan's and the
/// connect-fallback's shared "which rows can this sing-box even load"
/// check. Returns (loadable rows, ids sing-box refuses).
pub async fn isolate_unloadable_outbounds(
    app: &AppHandle,
    rows: Vec<OutboundRow>,
) -> Result<(Vec<OutboundRow>, Vec<i64>), String> {
    let api_port = free_local_port()?;
    let _ = app;
    tauri::async_runtime::spawn_blocking(move || {
        let check = |values: &[Value]| run_sing_box_check(api_port, values);
        isolate_bad_outbounds(&rows, &check)
    })
    .await
    .map_err(|error| format!("config validation failed: {error}"))?
}

/// Polls the clash API until it answers or the deadline passes.
pub fn wait_for_api(port: u16) -> bool {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(1)))
        .build()
        .into();
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        let up = matches!(
            agent.get(&format!("http://127.0.0.1:{port}/version")).call(),
            Ok(response) if response.status().as_u16() == 200
        );
        if up {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolate_bad_outbounds_bisects_until_offenders_are_found() {
        // even indices are poisoned; the check fails iff any poison is present
        let rows: Vec<OutboundRow> = (0..5)
            .map(|index| (index, json!({ "type": "test", "poison": index % 2 == 0 })))
            .collect();
        let check = |values: &[Value]| -> Result<bool, String> {
            Ok(values.iter().all(|value| value["poison"] != json!(true)))
        };

        let (good, broken) = isolate_bad_outbounds(&rows, &check).expect("isolate");

        assert_eq!(good.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(broken, vec![0, 2, 4]);
        // a clean set passes through untouched, a single bad row is isolated
        let clean: Vec<OutboundRow> = vec![(7, json!({ "type": "test" }))];
        let (good, broken) =
            isolate_bad_outbounds(&clean, &check).expect("isolate");
        assert_eq!(good.len(), 1);
        assert!(broken.is_empty());
        let single: Vec<OutboundRow> = vec![(8, json!({ "type": "test", "poison": true }))];
        let (good, broken) =
            isolate_bad_outbounds(&single, &check).expect("isolate");
        assert!(good.is_empty());
        assert_eq!(broken, vec![8]);
    }
}
