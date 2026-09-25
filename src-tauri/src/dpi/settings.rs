use rusqlite::Connection;
use serde::Serialize;

use crate::db::get_setting;

use super::strategies::db_err;

pub const DEFAULT_DPI_PORT: u16 = 1080;

pub(super) const SETTING_DPI_ENABLED: &str = "dpi_enabled";

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DpiSettings {
    pub port: u16,
    /// master toggle: off — the byedpi tunnel never spawns and a connect is
    /// proxy-only (dpi-routed categories fall through to the fallback)
    pub enabled: bool,
}

pub fn load_dpi_port(conn: &Connection) -> Result<u16, String> {
    let port = get_setting(conn, "dpi_port")
        .map_err(db_err)?
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(DEFAULT_DPI_PORT);
    Ok(port)
}

/// The DPI master toggle; junk/missing values fall back to on (DPI is part
/// of the shipped experience). Read by the connect flow: off — no ciadpi
/// spawn at all, dpi-routed traffic goes through the routing fallback.
pub fn dpi_enabled(conn: &Connection) -> Result<bool, String> {
    Ok(get_setting(conn, SETTING_DPI_ENABLED)
        .map_err(db_err)?
        .map(|value| value != "0")
        .unwrap_or(true))
}
