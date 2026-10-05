//! The session state: the `Running` record of a live session behind the
//! `STATE`/`FLOW` locks, the snapshot types the UI reads, and the shared
//! mode/tag constants of the generated config.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;

use crate::process::CommandChild;

use super::byedpi::DpiRunning;
use super::system_proxy::SystemProxyRestore;

pub const CONNECTION_CHANGED_EVENT: &str = "connection-changed";

pub(super) const MODE_OFF: &str = "off";
pub(super) const MODE_SYSTEM_PROXY: &str = "system-proxy";
pub(super) const MODE_TUN: &str = "tun";

/// Internal tags in the generated config: ASCII-only, collision-free.
pub(super) const PROXY_TAG: &str = "mt-proxy";
pub(super) const DIRECT_TAG: &str = "mt-direct";
pub(super) const DPI_TAG: &str = "mt-dpi";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionSnapshot {
    pub connected: bool,
    pub profile_id: Option<i64>,
    pub profile_name: Option<String>,
    pub endpoint_tag: Option<String>,
    pub mode: Option<String>,
    pub mixed_port: Option<u16>,
    /// the raw local proxy port (Settings → General), when the session
    /// bakes one — apps point their proxy setting at `127.0.0.1:<port>`
    pub raw_proxy_port: Option<u16>,
    /// present when the byedpi DPI tunnel runs alongside sing-box
    pub dpi: Option<DpiSnapshot>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DpiSnapshot {
    pub strategy: String,
    pub port: u16,
}

pub(super) struct Running {
    pub(super) child: Option<CommandChild>,
    pub(super) config_path: PathBuf,
    pub(super) profile_id: i64,
    pub(super) profile_name: String,
    /// `None` in a DPI-only session (no endpoint selected)
    pub(super) endpoint_tag: Option<String>,
    pub(super) mode: &'static str,
    pub(super) mixed_port: u16,
    /// the raw local proxy port this session bakes (`None` — off, or a
    /// DPI-only session with no selector to ride)
    pub(super) raw_port: Option<u16>,
    /// the clash API port of the running instance (live endpoint switches)
    pub(super) api_port: u16,
    /// endpoint row id → synthetic outbound tag baked into the config; a
    /// live switch resolves its target through this map
    pub(super) endpoint_tags: HashMap<i64, String>,
    pub(super) restore: SystemProxyRestore,
    pub(super) dpi: Option<DpiRunning>,
}

#[derive(Default)]
pub(super) struct ProxyState {
    pub(super) running: Option<Running>,
    pub(super) last_error: Option<String>,
}

pub(super) static STATE: LazyLock<Mutex<ProxyState>> =
    LazyLock::new(|| Mutex::new(ProxyState::default()));

/// Serializes whole connect/disconnect flows (the state mutex is only ever
/// held for short, await-free sections).
pub(super) static FLOW: LazyLock<AsyncMutex<()>> = LazyLock::new(|| AsyncMutex::new(()));

pub(super) fn lock_state() -> Result<MutexGuard<'static, ProxyState>, String> {
    STATE.lock().map_err(|_| "connection state lock poisoned".to_string())
}

pub(super) fn snapshot_of(state: &ProxyState) -> ConnectionSnapshot {
    match &state.running {
        Some(run) => ConnectionSnapshot {
            connected: true,
            profile_id: Some(run.profile_id),
            profile_name: Some(run.profile_name.clone()),
            endpoint_tag: run.endpoint_tag.clone(),
            mode: Some(run.mode.to_string()),
            mixed_port: Some(run.mixed_port),
            raw_proxy_port: run.raw_port,
            dpi: run.dpi.as_ref().map(|dpi| DpiSnapshot {
                strategy: dpi.strategy.clone(),
                port: dpi.port,
            }),
            last_error: state.last_error.clone(),
        },
        None => ConnectionSnapshot {
            last_error: state.last_error.clone(),
            ..ConnectionSnapshot::default()
        },
    }
}

pub(super) fn current_snapshot() -> ConnectionSnapshot {
    lock_state()
        .map(|state| snapshot_of(&state))
        .unwrap_or_else(|_| ConnectionSnapshot {
            last_error: Some("connection state lock poisoned".to_string()),
            ..ConnectionSnapshot::default()
        })
}
