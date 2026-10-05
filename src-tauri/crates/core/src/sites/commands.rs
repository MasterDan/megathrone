use rusqlite::{Connection, params};

use super::crud::{
    add_category, add_rule, category_rule_count, delete_category, delete_rule, rename_category,
    set_rule_test, update_rule,
};
use super::model::{CategoryRule, TestSiteCategory};
use super::queries::list_categories;
use super::set_action;
use super::validation::{validate_rule_type, validate_rule_value};
use crate::AppState;
use crate::dpi::validate_name;
use crate::env;
use crate::routing::validate_action;

/// Emitted after the category catalog changes (including routing action
/// flips), so the Settings sections and the strategy list refresh their views.
pub const TEST_SITES_CHANGED_EVENT: &str = "test-sites-changed";

// ---------------------------------------------------------------------------
// Tauri commands (thin wrappers over the storage layer)
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn sites_list() -> Result<Vec<TestSiteCategory>, String> {
    let state = env::state();
    let conn = lock_db(&state)?;
    list_categories(&conn)
}

#[tauri::command]
pub fn sites_add_category(name: String) -> Result<TestSiteCategory, String> {
    let name = validate_name(&name)?;
    let state = env::state();
    let conn = lock_db(&state)?;
    let added = add_category(&conn, &name)?;
    env::emit(TEST_SITES_CHANGED_EVENT, ());
    // an empty category contributes no rules — the session stays as is
    Ok(added)
}

#[tauri::command]
pub fn sites_rename_category(category_id: i64, name: String) -> Result<(), String> {
    let name = validate_name(&name)?;
    let state = env::state();
    let conn = lock_db(&state)?;
    rename_category(&conn, category_id, &name)?;
    env::emit(TEST_SITES_CHANGED_EVENT, ());
    // names never reach the generated sing-box config
    Ok(())
}

#[tauri::command]
pub async fn sites_delete_category(category_id: i64) -> Result<(), String> {
    let had_rules = {
        let state = env::state();
        let conn = lock_db(&state)?;
        let had_rules = category_rule_count(&conn, category_id)? > 0;
        delete_category(&conn, category_id)?;
        had_rules
    };
    env::emit(TEST_SITES_CHANGED_EVENT, ());
    if had_rules {
        crate::routing::apply_routing_change_live().await;
    }
    Ok(())
}

/// Points a category at another outbound (dpi | proxy | direct): what its
/// rules do while the proxy is connected, and which test probes them.
/// Idempotent. A running session restarts with the new route (only an
/// empty category leaves it untouched — no rules, no config).
#[tauri::command]
pub async fn sites_set_action(category_id: i64, action: String) -> Result<(), String> {
    let action = validate_action(&action)?;
    let (changed, has_rules) = {
        let state = env::state();
        let conn = lock_db(&state)?;
        let previous = conn
            .query_row(
                "SELECT action FROM test_site_categories WHERE id = ?1",
                params![category_id],
                |row| row.get::<_, String>(0),
            )
            .ok();
        let has_rules = category_rule_count(&conn, category_id)? > 0;
        set_action(&conn, category_id, &action)?;
        (previous.as_deref() != Some(action.as_str()), has_rules)
    };
    env::emit(TEST_SITES_CHANGED_EVENT, ());
    if changed && has_rules {
        crate::routing::apply_routing_change_live().await;
    }
    Ok(())
}

#[tauri::command]
pub async fn sites_add_rule(
    category_id: i64,
    rule_type: String,
    value: String,
) -> Result<CategoryRule, String> {
    let rule_type = validate_rule_type(&rule_type)?;
    let value = validate_rule_value(&rule_type, &value)?;
    let added = {
        let state = env::state();
        let conn = lock_db(&state)?;
        add_rule(&conn, category_id, &rule_type, &value)?
    };
    env::emit(TEST_SITES_CHANGED_EVENT, ());
    crate::routing::apply_routing_change_live().await;
    Ok(added)
}

#[tauri::command]
pub async fn sites_update_rule(
    rule_id: i64,
    rule_type: String,
    value: String,
) -> Result<(), String> {
    let rule_type = validate_rule_type(&rule_type)?;
    let value = validate_rule_value(&rule_type, &value)?;
    {
        let state = env::state();
        let conn = lock_db(&state)?;
        update_rule(&conn, rule_id, &rule_type, &value)?;
    }
    env::emit(TEST_SITES_CHANGED_EVENT, ());
    crate::routing::apply_routing_change_live().await;
    Ok(())
}

/// Toggles whether a rule joins the DPI strategy test and the endpoint
/// deep probe. Idempotent. Testing only — the session keeps its route.
#[tauri::command]
pub fn sites_set_rule_test(rule_id: i64, test_enabled: bool) -> Result<(), String> {
    let state = env::state();
    let conn = lock_db(&state)?;
    set_rule_test(&conn, rule_id, test_enabled)?;
    env::emit(TEST_SITES_CHANGED_EVENT, ());
    Ok(())
}

#[tauri::command]
pub async fn sites_delete_rule(rule_id: i64) -> Result<(), String> {
    {
        let state = env::state();
        let conn = lock_db(&state)?;
        delete_rule(&conn, rule_id)?;
    }
    env::emit(TEST_SITES_CHANGED_EVENT, ());
    crate::routing::apply_routing_change_live().await;
    Ok(())
}

fn lock_db(state: &AppState) -> Result<std::sync::MutexGuard<'_, Connection>, String> {
    state.db.lock().map_err(|_| "database lock poisoned".to_string())
}
