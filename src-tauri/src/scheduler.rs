//! Background auto-update scheduler.
//!
//! A single long-lived task wakes either when the next profile update is due
//! or when something mutates the profiles (see `AppState::notify_auto_update`).
//! There is no polling loop: the sleep target comes from the DB
//! (`next_wake_seconds`), and every pass recomputes it.

use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Manager};
use tokio::sync::Notify;

use crate::AppState;
use crate::profiles;

/// Never spin faster than this, even if a schedule is overdue.
const MIN_WAIT_SECONDS: f64 = 30.0;
/// Nothing scheduled — just wait for external nudges (checks are cheap).
const IDLE_WAIT_SECONDS: f64 = 6.0 * 3600.0;
/// A failed pass retries no sooner than this to avoid a hot loop on a
/// permanently overdue-but-failing profile.
const FAILURE_BACKOFF_SECONDS: f64 = 300.0;

pub fn spawn(app: AppHandle, notify: Arc<Notify>) {
    tauri::async_runtime::spawn(async move {
        loop {
            let pass_app = app.clone();
            let pass = tauri::async_runtime::spawn_blocking(move || run_pass(&pass_app)).await;

            let (next_in, failures) = match pass {
                Ok(Ok(result)) => result,
                Ok(Err(error)) => {
                    eprintln!("[auto-update] pass failed: {error}");
                    (None, 1)
                }
                Err(error) => {
                    eprintln!("[auto-update] pass panicked: {error}");
                    (None, 1)
                }
            };

            let wait = next_in.unwrap_or(IDLE_WAIT_SECONDS).max(MIN_WAIT_SECONDS)
                + f64::from(u8::from(failures > 0)) * FAILURE_BACKOFF_SECONDS;

            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs_f64(wait)) => {}
                _ = notify.notified() => {}
            }
        }
    });
}

/// Updates everything that is due and reports when the next update is due.
fn run_pass(app: &AppHandle) -> Result<(Option<f64>, usize), String> {
    let due = {
        let state = app.state::<AppState>();
        let conn = state
            .db
            .lock()
            .map_err(|_| "database lock poisoned".to_string())?;
        profiles::due_profiles(&conn)?
    };

    let mut failures = 0;
    for profile_id in due {
        match profiles::update_profile_flow(app, profile_id) {
            Ok(outcome) => {
                // the endpoint set changed under a live session's feet —
                // rebuild it silently (the manual Update button reports
                // its rebuild through the restart toast instead)
                if outcome.set_changed {
                    let restart_app = app.clone();
                    tauri::async_runtime::spawn(profiles::rebuild_session_after_update(
                        restart_app,
                        profile_id,
                        true,
                        false,
                    ));
                }
            }
            Err(error) => {
                eprintln!("[auto-update] profile {profile_id} failed: {error}");
                failures += 1;
            }
        }
    }

    let next = {
        let state = app.state::<AppState>();
        let conn = state
            .db
            .lock()
            .map_err(|_| "database lock poisoned".to_string())?;
        profiles::next_wake_seconds(&conn)?
    };

    Ok((next, failures))
}
