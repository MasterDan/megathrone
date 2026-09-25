use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use serde::Serialize;

/// In-flight scans keyed by profile id: live progress (so a re-mounted page
/// can adopt a running scan via `profile_latency_status`) plus the cancel flag.
struct ScanState {
    done: usize,
    total: usize,
    cancel: Arc<AtomicBool>,
}

static SCANS: LazyLock<Mutex<HashMap<i64, ScanState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock_scans() -> Result<MutexGuard<'static, HashMap<i64, ScanState>>, String> {
    SCANS.lock().map_err(|_| "scan registry poisoned".to_string())
}

/// Removes the profile's registry entry when the scan ends for any reason,
/// including early returns and panics.
pub(super) struct ScanGuard(pub(super) i64);

impl Drop for ScanGuard {
    fn drop(&mut self) {
        if let Ok(mut scans) = SCANS.lock() {
            scans.remove(&self.0);
        }
    }
}

pub(super) fn begin_scan(profile_id: i64, total: usize) -> Result<Arc<AtomicBool>, String> {
    let mut scans = lock_scans()?;
    if scans.contains_key(&profile_id) {
        return Err(format!("a latency scan is already running for profile {profile_id}"));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    scans.insert(profile_id, ScanState { done: 0, total, cancel: cancel.clone() });
    Ok(cancel)
}

pub(super) fn advance_scan(profile_id: i64, done: usize) {
    if let Ok(mut scans) = SCANS.lock() {
        if let Some(state) = scans.get_mut(&profile_id) {
            state.done = done;
        }
    }
}

fn request_cancel(profile_id: i64) -> Result<bool, String> {
    let scans = lock_scans()?;
    match scans.get(&profile_id) {
        Some(state) => {
            state.cancel.store(true, Ordering::Relaxed);
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Stops a running scan for the profile and (blocking) waits until it has
/// fully exited — the profile update flow calls this before rewriting the
/// endpoint rows under a scan's feet. The cancel is cooperative (checked
/// between batches), so the wait is bounded by one batch. Returns false
/// when the scan outlived the timeout; the caller proceeds anyway and the
/// FK-tolerant writes cover the residual race.
pub fn cancel_and_wait(profile_id: i64, timeout: Duration) -> bool {
    if !request_cancel(profile_id).unwrap_or(false) {
        return true; // nothing to wait for
    }
    let deadline = std::time::Instant::now() + timeout;
    while scan_running(profile_id) {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LatencyStatus {
    pub done: usize,
    pub total: usize,
}

/// Snapshot of a profile's scan; `None` means no scan is running.
#[tauri::command]
pub fn profile_latency_status(profile_id: i64) -> Result<Option<LatencyStatus>, String> {
    let scans = lock_scans()?;
    Ok(scans
        .get(&profile_id)
        .map(|state| LatencyStatus { done: state.done, total: state.total }))
}

/// Whether a scan for this profile is in flight right now (the supervisor
/// waits these out and reuses their data).
pub fn scan_running(profile_id: i64) -> bool {
    lock_scans().map(|scans| scans.contains_key(&profile_id)).unwrap_or(false)
}

/// Stops a running scan after the current batch; idempotent.
#[tauri::command]
pub fn profile_cancel_latency(profile_id: i64) -> Result<bool, String> {
    request_cancel(profile_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_registry_tracks_and_cancels() {
        // unique ids: the registry is a process-wide static and tests run in parallel
        let profile_a = 900_001i64;
        let profile_b = 900_002i64;

        let cancel = begin_scan(profile_a, 10).expect("begin scan");
        assert!(begin_scan(profile_a, 20).is_err(), "a second scan must be rejected");
        let cancel_b = begin_scan(profile_b, 5).expect("begin scan b");

        advance_scan(profile_a, 4);
        assert_eq!(
            profile_latency_status(profile_a).expect("status"),
            Some(LatencyStatus { done: 4, total: 10 })
        );
        assert_eq!(
            profile_latency_status(profile_b).expect("status"),
            Some(LatencyStatus { done: 0, total: 5 })
        );

        assert!(!cancel.load(Ordering::Relaxed));
        assert!(request_cancel(profile_a).expect("cancel"), "running scan reported cancelled");
        assert!(cancel.load(Ordering::Relaxed), "the worker must observe the flag");
        assert!(request_cancel(profile_a).expect("cancel"), "cancel is idempotent");
        assert!(!cancel_b.load(Ordering::Relaxed), "profiles are independent");

        drop(ScanGuard(profile_a));
        assert_eq!(profile_latency_status(profile_a).expect("status"), None, "guard cleans the entry");
        assert!(!request_cancel(999_999).expect("cancel"), "no scan, nothing to cancel");

        drop(ScanGuard(profile_b));
        assert_eq!(profile_latency_status(profile_b).expect("status"), None);
    }

    #[test]
    fn cancel_and_wait_blocks_until_the_scan_exits() {
        // unique id: the registry is a process-wide static
        let profile = 900_101i64;

        // no scan — nothing to wait for
        assert!(cancel_and_wait(profile, Duration::from_millis(10)));

        // a scan that exits on its own after a moment: the wait observes it
        let _cancel = begin_scan(profile, 10).expect("begin scan");
        let exiter = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            drop(ScanGuard(profile)); // what a real scan's exit path does
        });
        let started = std::time::Instant::now();
        assert!(
            cancel_and_wait(profile, Duration::from_secs(5)),
            "the scan exited within the timeout"
        );
        assert!(
            started.elapsed() >= Duration::from_millis(100),
            "the wait actually blocked until the exit"
        );
        exiter.join().expect("exiter");

        // a scan that never exits: the timeout fires (proceed-anyway path)
        let _held = begin_scan(profile, 3).expect("begin scan again");
        let started = std::time::Instant::now();
        assert!(
            !cancel_and_wait(profile, Duration::from_millis(80)),
            "a stuck scan times out"
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(ScanGuard(profile));
    }
}
