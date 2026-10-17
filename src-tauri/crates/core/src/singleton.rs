//! One Megathrone process per user session: the desktop app and the console
//! web server share one database in one data directory, so a second
//! instance of either must never run concurrently — the scheduler, the
//! sidecar sessions and the system proxy integration are per-machine
//! singletons. The guard is an exclusively locked file (std's
//! `File::try_lock` — `flock` on Unix, `LockFileEx` on Windows) in the data
//! directory: the OS releases it when the holder dies, so a crash never
//! leaves a stale lock behind.

use std::fmt;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// The app identifier — must match `identifier` in `tauri.conf.json`:
/// [`canonical_data_dir`] has to resolve to the exact directory the Tauri
/// shell's `app_data_dir()` points at (`identifier_matches_tauri_conf`
/// enforces the sync).
pub const APP_IDENTIFIER: &str = "com.megathrone.app";

/// The lock file inside the data directory.
const LOCK_FILE_NAME: &str = "megathrone.lock";

/// A held instance lock. Dropping the guard closes the file and releases
/// the lock — the shells keep it for the whole process lifetime
/// (`std::mem::forget`).
#[must_use = "dropping the guard releases the instance lock"]
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

/// Why the single-instance lock could not be taken.
#[derive(Debug)]
pub enum SingletonError {
    /// The OS data directory cannot be resolved.
    NoDataDir,
    /// The lock file could not be created or locked.
    Io(String),
    /// Another process holds the lock (its PID when it could be read).
    Held { pid: Option<u32> },
}

impl fmt::Display for SingletonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDataDir => write!(f, "cannot resolve the OS data directory"),
            Self::Io(context) => write!(f, "failed to acquire the instance lock: {context}"),
            Self::Held {
                pid: Some(pid),
            } => write!(f, "another Megathrone instance is already running (pid {pid})"),
            Self::Held { pid: None } => {
                write!(f, "another Megathrone instance is already running")
            }
        }
    }
}

impl std::error::Error for SingletonError {}

/// The canonical Megathrone data directory: `<OS data dir>/<identifier>` —
/// the same directory the Tauri shell's `app_data_dir()` resolves to, the
/// single home of the shared database and of the instance lock.
pub fn canonical_data_dir() -> Result<PathBuf, SingletonError> {
    dirs::data_dir()
        .map(|dir| dir.join(APP_IDENTIFIER))
        .ok_or(SingletonError::NoDataDir)
}

/// Takes the single-instance lock of `data_dir` (creating the directory and
/// the lock file as needed): an exclusive non-blocking lock plus the
/// holder's PID stamped into the file for whoever is refused next. The
/// Tauri shell passes its `app_data_dir()`, the web server the canonical
/// dir — one lock file guards both.
pub fn acquire(data_dir: &Path) -> Result<InstanceLock, SingletonError> {
    std::fs::create_dir_all(data_dir)
        .map_err(|e| SingletonError::Io(format!("cannot create {}: {e}", data_dir.display())))?;
    let path = data_dir.join(LOCK_FILE_NAME);
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        // no truncate: clobbering here would wipe the holder's stamped pid
        // before the lock is even attempted
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| SingletonError::Io(format!("cannot open {}: {e}", path.display())))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            return Err(SingletonError::Held {
                pid: read_holder_pid(&path),
            });
        }
        Err(e) => {
            return Err(SingletonError::Io(format!("cannot lock {}: {e}", path.display())));
        }
    }
    // stamp the holder's pid (truncate first: a shorter pid than the
    // previous holder's would leave digits behind)
    if let Err(e) = file
        .set_len(0)
        .and_then(|()| file.write_all(std::process::id().to_string().as_bytes()))
    {
        return Err(SingletonError::Io(format!(
            "cannot stamp the pid into {}: {e}",
            path.display()
        )));
    }
    Ok(InstanceLock { _file: file })
}

/// Reads the PID the current lock holder stamped into the file (best
/// effort — the value only aids the refusal message).
fn read_holder_pid(path: &Path) -> Option<u32> {
    let mut content = String::new();
    File::open(path).ok()?.read_to_string(&mut content).ok()?;
    content.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("megathrone-singleton-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn lock_is_exclusive_until_released() {
        let dir = scratch("exclusive");
        let first = acquire(&dir).expect("the first acquisition takes the lock");
        // flock conflicts are per open file description, so even a second
        // open in the same process must be refused — exactly what a
        // concurrent second instance sees
        let second = acquire(&dir);
        assert!(
            matches!(second, Err(SingletonError::Held { pid: Some(_) })),
            "a concurrent acquisition must be refused as Held with the holder's pid, got {second:?}"
        );
        drop(first);
        drop(
            acquire(&dir).expect("releasing the guard frees the lock for the next holder"),
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lock_file_stamps_the_holder_pid() {
        let dir = scratch("pid");
        let lock = acquire(&dir).expect("the lock is taken");
        let content =
            std::fs::read_to_string(dir.join(LOCK_FILE_NAME)).expect("the lock file exists");
        assert_eq!(content.trim(), std::process::id().to_string());
        drop(lock);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn canonical_dir_points_at_the_tauri_identifier() {
        let os_dir = dirs::data_dir().expect("the OS data dir resolves in tests");
        assert_eq!(
            canonical_data_dir().expect("the canonical dir resolves"),
            os_dir.join(APP_IDENTIFIER)
        );
    }

    #[test]
    fn identifier_matches_tauri_conf() {
        let conf = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tauri.conf.json");
        let raw = std::fs::read_to_string(&conf).expect("tauri.conf.json is reachable");
        let value: serde_json::Value =
            serde_json::from_str(&raw).expect("tauri.conf.json parses");
        assert_eq!(
            value["identifier"].as_str(),
            Some(APP_IDENTIFIER),
            "singleton::APP_IDENTIFIER drifted from tauri.conf.json — the lock and the app data dir would split"
        );
    }
}
