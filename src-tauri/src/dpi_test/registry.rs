use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use serde::Serialize;

// ---------------------------------------------------------------------------
// Test registry (one running test per app)
// ---------------------------------------------------------------------------

struct TestRun {
    done: usize,
    total: usize,
    cancel: Arc<AtomicBool>,
}

static TEST: LazyLock<Mutex<Option<TestRun>>> = LazyLock::new(|| Mutex::new(None));

fn lock_test() -> Result<MutexGuard<'static, Option<TestRun>>, String> {
    TEST.lock().map_err(|_| "test registry poisoned".to_string())
}

/// Removes the registry entry when the test ends for any reason, including
/// early returns and panics.
pub(super) struct TestGuard;

impl Drop for TestGuard {
    fn drop(&mut self) {
        if let Ok(mut test) = TEST.lock() {
            *test = None;
        }
    }
}

pub(super) fn begin_test(total: usize) -> Result<Arc<AtomicBool>, String> {
    let mut test = lock_test()?;
    if test.is_some() {
        return Err("a DPI strategy test is already running".to_string());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    *test = Some(TestRun { done: 0, total, cancel: cancel.clone() });
    Ok(cancel)
}

pub(super) fn advance_test(done: usize) {
    if let Ok(mut test) = TEST.lock() {
        if let Some(run) = test.as_mut() {
            run.done = done;
        }
    }
}

fn request_cancel() -> Result<bool, String> {
    let test = lock_test()?;
    match test.as_ref() {
        Some(run) => {
            run.cancel.store(true, Ordering::Relaxed);
            Ok(true)
        }
        None => Ok(false),
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DpiTestStatus {
    pub done: usize,
    pub total: usize,
}

/// Snapshot of the running test; `None` means no test is running.
#[tauri::command]
pub fn dpi_test_status() -> Result<Option<DpiTestStatus>, String> {
    let test = lock_test()?;
    Ok(test
        .as_ref()
        .map(|run| DpiTestStatus { done: run.done, total: run.total }))
}

/// Stops a running test after the current strategy; idempotent.
#[tauri::command]
pub fn dpi_cancel_test() -> Result<bool, String> {
    request_cancel()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_tracks_and_cancels() {
        let cancel = begin_test(10).expect("begin test");
        assert!(begin_test(20).is_err(), "a second test must be rejected");

        advance_test(4);
        assert_eq!(dpi_test_status().expect("status"), Some(DpiTestStatus { done: 4, total: 10 }));

        assert!(!cancel.load(Ordering::Relaxed));
        assert!(request_cancel().expect("cancel"));
        assert!(cancel.load(Ordering::Relaxed));
        assert!(request_cancel().expect("cancel"), "cancel is idempotent");

        drop(TestGuard);
        assert_eq!(dpi_test_status().expect("status"), None);
        assert!(!request_cancel().expect("cancel"), "no test, nothing to cancel");

        // the slot is free again after a finished test
        begin_test(1).expect("begin again");
        drop(TestGuard);
        assert_eq!(dpi_test_status().expect("status"), None);
    }
}
