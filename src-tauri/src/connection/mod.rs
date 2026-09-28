//! The long-lived proxy connection: runs sing-box with *every* endpoint of
//! the profile baked into the config behind a `selector` outbound (tag
//! `mt-proxy`, default = the selected endpoint) — so an endpoint change
//! (strategy rotation, rescue, a manual pick) is one Clash API call on the
//! running instance, no restart: the port, TUN device and system proxy stay
//! put, new connections ride the new endpoint immediately and open ones
//! drain on the old one. Only changes that alter the config itself (fresh
//! content, routing, DPI, mode, profile) reconnect. On top of that:
//! optional traffic splitting (Routing settings): every URL category's
//! hosts go through the proxy, the
//! byedpi DPI tunnel (a local ciadpi SOCKS server spawned alongside) or
//! direct, with a fallback action for everything else. Without a usable
//! selection the connect flow picks an endpoint automatically per the
//! profile's strategy (round robin / fastest / most available — see
//! `auto_select`); only a profile with nothing dialable at all falls back to
//! a DPI-only session (active strategy + DPI routing: no proxy outbound,
//! DPI rules intact, everything else direct). Traffic reaches sing-box
//! either through a local mixed port, the system proxy (macOS, Windows and
//! GNOME/KDE Linux desktops), or a TUN device. On top of the session port, the raw local proxy (Settings →
//! General, on by default) is a second mixed port whose rule rides above
//! every routing rule: apps that support a proxy point at
//! `127.0.0.1:<port>` and send *everything* — private ranges included —
//! through the currently selected endpoint, following its live switches.
//! The instance is watched: if it dies on its own, the UI hears
//! about it via `connection-changed` together with the stderr tail (this is
//! how e.g. missing TUN permissions surface).

// Split by concern: `state` the session state and snapshot types behind the
// `STATE`/`FLOW` locks, `connect`/`teardown` the session lifecycle (connect
// flow, disconnect, watcher, shutdown), `switching` the live
// reconfiguration paths (endpoint/profile switch, restart-result toast),
// `status` the read-only queries, `selection` the endpoint-picking storage
// layer, `config` the generated sing-box run config, `byedpi` the DPI
// sidecar spawn/probe/watch, `startup` the sing-box startup probe and
// privilege hints, `traffic` the clash `/connections` poller with the
// per-URL stats, `system_proxy` the per-OS system proxy wiring and
// `recovery` the crash-recovery marker. The glob re-exports keep the
// historical `crate::connection::*` surface intact — the command globs also
// carry the hidden `__cmd__*` macro bindings `generate_handler!` looks up
// here. Per the repo convention this file stays re-exports only.

mod byedpi;
mod config;
mod connect;
mod recovery;
mod selection;
mod startup;
mod state;
mod status;
mod switching;
mod system_proxy;
mod teardown;
mod traffic;

#[cfg(test)]
mod config_tests;
#[cfg(test)]
mod connection_tests;
#[cfg(test)]
mod selection_tests;

pub(crate) use byedpi::probe_dpi_ready;
pub use connect::*;
pub use recovery::{install_exit_signals, recover};
// the snapshot re-exports feed the desktop-only tray module — on android
// nothing consumes the path
#[cfg(desktop)]
pub use state::{ConnectionSnapshot, CONNECTION_CHANGED_EVENT};
pub use status::*;
pub use switching::*;
pub use teardown::*;
pub use traffic::*;
