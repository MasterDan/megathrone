//! Traffic-splitting while the proxy is connected — driven entirely by the
//! URL categories (Settings → Routing): every category carries an
//! action (dpi | proxy | direct, switched in the Routing list) and all of
//! its hosts follow it in the generated sing-box route. The fallback action
//! covers everything no category matched (the historical behavior of the
//! app: everything through the proxy).

use rusqlite::Connection;
use serde::Serialize;
use tauri::{AppHandle, State};

use crate::AppState;
use crate::connection;
use crate::db::{get_setting, set_setting};
use crate::sites;

pub const ACTION_PROXY: &str = "proxy";
pub const ACTION_DPI: &str = "dpi";
pub const ACTION_DIRECT: &str = "direct";
pub const ACTIONS: [&str; 3] = [ACTION_PROXY, ACTION_DPI, ACTION_DIRECT];

const SETTING_FALLBACK: &str = "route_fallback";

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RouteRule {
    /// url | domain_suffix | domain | domain_keyword | domain_regex
    pub rule_type: String,
    pub value: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RouteCategory {
    pub id: i64,
    pub name: String,
    /// proxy | dpi | direct
    pub action: String,
    /// the category's typed routing rules (`url` rules included — the
    /// connect flow routes them by their hostname)
    pub rules: Vec<RouteRule>,
}

/// Everything a connect needs: the categories with their routing actions
/// and the fallback action for everything unmatched.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RoutingConfig {
    pub categories: Vec<RouteCategory>,
    pub fallback: String,
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn routing_list(state: State<AppState>) -> Result<RoutingConfig, String> {
    let conn = lock_db(&state)?;
    load_routing(&conn)
}

/// Changes the fallback action for everything unmatched. A running session
/// restarts with the new route — the outcome lands in the global toast.
#[tauri::command]
pub async fn routing_set_fallback(
    app: AppHandle,
    state: State<'_, AppState>,
    action: String,
) -> Result<(), String> {
    let action = validate_action(&action)?;
    let changed = {
        let conn = lock_db(&state)?;
        let previous = get_setting(&conn, SETTING_FALLBACK)
            .map_err(db_err)?
            .filter(|value| ACTIONS.contains(&value.as_str()));
        set_setting(&conn, SETTING_FALLBACK, &action).map_err(db_err)?;
        previous.as_deref() != Some(action.as_str())
    };
    if changed {
        apply_routing_change_live(&app).await;
    }
    Ok(())
}

/// A routing-affecting setting just changed (a rule/category/fallback
/// mutation): rebuild the live session so the new route rules apply. The
/// outcome lands in the global toast, like every user-initiated
/// reconfiguration. Callers gate on the change actually reaching the
/// config (empty categories, renames and test toggles do not).
pub async fn apply_routing_change_live(app: &AppHandle) {
    let Some(profile_id) = connection::running_profile() else {
        return;
    };
    match connection::restart_if_running(app.clone(), profile_id).await {
        Ok(Some(_)) | Ok(None) => {
            // Ok(None) after the running check: a DPI-only session — the
            // route rules were rebuilt just the same
            let snapshot = connection::snapshot();
            let message = match &snapshot.dpi {
                Some(dpi) => format!("routing applied — DPI tunnel: {}", dpi.strategy),
                None => "routing applied".to_string(),
            };
            connection::emit_restart_result(app, true, message);
        }
        Err(error) => connection::emit_restart_result(app, false, error),
    }
}

// ---------------------------------------------------------------------------
// Storage layer
// ---------------------------------------------------------------------------

/// Categories + fallback, with broken rows (hand-edited DBs) skipped
/// silently — routing must never break the connect flow.
pub fn load_routing(conn: &Connection) -> Result<RoutingConfig, String> {
    let categories = sites::category_rules(conn)?
        .into_iter()
        .filter(|category| ACTIONS.contains(&category.action.as_str()))
        .map(|category| RouteCategory {
            id: category.id,
            name: category.name,
            action: category.action,
            rules: category
                .rules
                .into_iter()
                .filter(|rule| sites::RULE_TYPES.contains(&rule.rule_type.as_str()))
                .map(|rule| RouteRule { rule_type: rule.rule_type, value: rule.value })
                .collect(),
        })
        .collect();

    let fallback = get_setting(conn, SETTING_FALLBACK)
        .map_err(db_err)?
        .filter(|value| ACTIONS.contains(&value.as_str()))
        .unwrap_or_else(|| ACTION_PROXY.to_string());

    Ok(RoutingConfig { categories, fallback })
}

/// True when at least one dpi-routed category or the fallback needs the DPI
/// tunnel.
pub fn dpi_used(routing: &RoutingConfig) -> bool {
    routing.fallback == ACTION_DPI
        || routing.categories.iter().any(|category| category.action == ACTION_DPI)
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

pub(crate) fn validate_action(value: &str) -> Result<String, String> {
    match value {
        ACTION_PROXY | ACTION_DPI | ACTION_DIRECT => Ok(value.to_string()),
        other => Err(format!("unknown action `{other}` (expected one of: {ACTIONS:?})")),
    }
}

fn lock_db<'a>(state: &'a State<'_, AppState>) -> Result<std::sync::MutexGuard<'a, Connection>, String> {
    state.db.lock().map_err(|_| "database lock poisoned".to_string())
}

fn db_err(error: rusqlite::Error) -> String {
    format!("database error: {error}")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use rusqlite::params;

    use super::*;

    fn test_db() -> Connection {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "megathrone-routing-{}-{id}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        crate::db::open(&path, &crate::db::Migrations::embedded()).expect("test db should open")
    }

    #[test]
    fn actions_are_whitelisted() {
        assert!(validate_action(ACTION_DPI).is_ok());
        assert!(validate_action("tun").is_err());
    }

    #[test]
    fn routing_roundtrip_and_fallback() {
        let conn = test_db();

        // default fallback is the historical behavior: everything proxied
        let config = load_routing(&conn).expect("load");
        assert_eq!(config.fallback, ACTION_PROXY);
        assert!(config.categories.is_empty());
        assert!(!dpi_used(&config));

        set_setting(&conn, SETTING_FALLBACK, ACTION_DIRECT).expect("fallback");
        let config = load_routing(&conn).expect("load");
        assert_eq!(config.fallback, ACTION_DIRECT);

        set_setting(&conn, SETTING_FALLBACK, ACTION_DPI).expect("fallback dpi");
        let config = load_routing(&conn).expect("load");
        assert!(dpi_used(&config), "a dpi fallback needs the tunnel");

        // junk falls back to proxy instead of breaking the connect flow
        set_setting(&conn, SETTING_FALLBACK, "blackhole").expect("junk fallback");
        let config = load_routing(&conn).expect("load");
        assert_eq!(config.fallback, ACTION_PROXY);
    }

    #[test]
    fn categories_join_the_effective_config_by_action() {
        let conn = test_db();
        crate::sites::seed_default_sites(&conn).expect("seed");

        let config = load_routing(&conn).expect("load");
        assert_eq!(config.categories.len(), 8, "every seeded category rides along");
        assert!(config.categories.iter().all(|category| category.action == ACTION_DPI));
        assert!(config.categories.iter().all(|category| !category.rules.is_empty()));
        assert!(dpi_used(&config), "a dpi category needs the tunnel");

        // one category to proxy — it stays in the config with its action
        let first = config.categories[0].id;
        crate::sites::set_action(&conn, first, ACTION_PROXY).expect("switch");
        let config = load_routing(&conn).expect("load");
        let switched = config.categories.iter().find(|c| c.id == first).expect("still listed");
        assert_eq!(switched.action, ACTION_PROXY);
        assert!(dpi_used(&config), "the other dpi categories still need it");

        // everything off proxy-routing — routing no longer needs the tunnel
        conn.execute(
            "UPDATE test_site_categories SET action = 'direct'",
            params![],
        )
        .expect("direct all");
        let config = load_routing(&conn).expect("load");
        assert!(config.categories.iter().all(|category| category.action == ACTION_DIRECT));
        assert!(!dpi_used(&config), "no dpi categories, proxy fallback");
    }

    #[test]
    fn broken_rows_are_skipped_not_fatal() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO test_site_categories (name, position, action) VALUES ('Junk', 0, 'tun')",
            [],
        )
        .expect("seed junk");

        let config = load_routing(&conn).expect("load");
        assert!(config.categories.is_empty(), "unknown actions are skipped");
    }
}
