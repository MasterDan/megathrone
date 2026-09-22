//! URL categories (Settings → Routing) — the single catalog both
//! tests and routing draw from.
//!
//! Every category carries a routing action (`dpi` | `proxy` | `direct`,
//! switched in the Routing list) and a list of typed routing rules.
//! `dpi` categories are probed by the DPI strategy test (Settings → DPI)
//! and their rules route into the DPI tunnel while the proxy is connected;
//! `proxy` categories are deep-probed through endpoints during latency
//! scans (`latency.rs`) and their rules go through the proxy; `direct`
//! categories match traffic that bypasses both. Only rules marked
//! `test_enabled` join the tests — and only rule types that can be turned
//! into a probe URL (`url`, `domain_suffix`, `domain`) at that. The
//! default catalog is adapted from the ByeByeDPI Android app's
//! `proxytest_*.sites` assets (github.com/romanvht/ByeByeDPI). Deleting a
//! category (or a single rule) cascades the matching result rows away, so
//! strategy percentages and endpoint scores always reflect the currently
//! configured list.

use rusqlite::{Connection, params};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::AppState;
use crate::dpi::validate_name;
use crate::routing::{ACTION_DPI, validate_action};
use crate::settings::validate_test_url;

/// Emitted after the category catalog changes (including routing action
/// flips), so the Settings sections and the strategy list refresh their views.
pub const TEST_SITES_CHANGED_EVENT: &str = "test-sites-changed";

pub const MAX_CATEGORIES: usize = 24;
pub const MAX_RULES_PER_CATEGORY: usize = 128;

/// Rule types a category member can be. Everything but `url` is a sing-box
/// domain matcher verbatim; `url` is a full test URL that routes by its
/// hostname (as a domain suffix).
pub const RULE_TYPE_URL: &str = "url";
pub const RULE_TYPE_DOMAIN_SUFFIX: &str = "domain_suffix";
pub const RULE_TYPE_DOMAIN: &str = "domain";
pub const RULE_TYPE_KEYWORD: &str = "domain_keyword";
pub const RULE_TYPE_REGEX: &str = "domain_regex";
pub const RULE_TYPES: [&str; 5] = [
    RULE_TYPE_DOMAIN_SUFFIX,
    RULE_TYPE_DOMAIN,
    RULE_TYPE_KEYWORD,
    RULE_TYPE_REGEX,
    RULE_TYPE_URL,
];

/// The default catalog: (category name, [bare hosts]). Hosts are stored as
/// `domain_suffix` rules — one rule covers the host and every subdomain.
const DEFAULT_SITES: &[(&str, &[&str])] = &[
    (
        "General",
        &["rutracker.org", "nyaa.si", "rutor.org", "nnmclub.to", "speedtest.net", "ookla.com"],
    ),
    (
        "Cloudflare",
        &["cloudflare.net", "cloudflare.com", "cloudflarecn.net", "cloudflare-ech.com"],
    ),
    (
        "Discord",
        &[
            "dis.gd",
            "discord.co",
            "discord.gg",
            "discord.app",
            "discord.com",
            "discord.dev",
            "discord.new",
            "discord.gift",
            "discord.gifts",
            "discord.media",
            "discord.store",
            "discord.design",
            "discordapp.com",
            "discordcdn.com",
            "discordsez.com",
            "discordsays.com",
            "discordmerch.com",
            "discordpartygames.com",
            "discordactivities.com",
            "stable.dl2.discordapp.net",
            "discord-attachments-uploads-prd.storage.googleapis.com",
        ],
    ),
    (
        "Google Video",
        &[
            "rr1---sn-4axm-n8vs.googlevideo.com",
            "rr1---sn-gvnuxaxjvh-o8ge.googlevideo.com",
            "rr1---sn-ug5onuxaxjvh-p3ul.googlevideo.com",
            "rr1---sn-ug5onuxaxjvh-n8v6.googlevideo.com",
            "rr4---sn-q4flrnsl.googlevideo.com",
            "rr10---sn-gvnuxaxjvh-304z.googlevideo.com",
            "rr14---sn-n8v7kn7r.googlevideo.com",
            "rr16---sn-axq7sn76.googlevideo.com",
            "rr1---sn-8ph2xajvh-5xge.googlevideo.com",
            "rr1---sn-gvnuxaxjvh-5gie.googlevideo.com",
            "rr12---sn-gvnuxaxjvh-bvwz.googlevideo.com",
            "rr5---sn-n8v7knez.googlevideo.com",
            "rr1---sn-u5uuxaxjvhg0-ocje.googlevideo.com",
            "rr2---sn-q4fl6ndl.googlevideo.com",
            "rr5---sn-gvnuxaxjvh-n8vk.googlevideo.com",
            "rr4---sn-jvhnu5g-c35d.googlevideo.com",
            "rr1---sn-q4fl6n6y.googlevideo.com",
            "rr2---sn-hgn7ynek.googlevideo.com",
            "rr1---sn-xguxaxjvh-gufl.googlevideo.com",
            "googlevideo.com",
        ],
    ),
    (
        "Social",
        &[
            "snapchat.com",
            "snap.com",
            "linkedin.com",
            "facebook.com",
            "fb.com",
            "fb.me",
            "fbcdn.net",
            "messenger.com",
            "meta.com",
            "instagram.com",
            "static.cdninstagram.com",
            "proton.me",
            "medium.com",
            "x.com",
            "twitter.com",
            "twimg.com",
            "soundcloud.com",
        ],
    ),
    (
        "Telegram",
        &[
            "telegram.org",
            "core.telegram.org",
            "web.telegram.org",
            "webk.telegram.org",
            "my.telegram.org",
            "translations.telegram.org",
            "instantview.telegram.org",
            "blog.telegram.org",
            "comments.telegram.org",
            "verify.telegram.org",
            "login.telegram.org",
            "auth.telegram.org",
            "api.telegram.org",
            "promo.telegram.org",
            "desktop.telegram.org",
            "macos.telegram.org",
            "ios.telegram.org",
            "android.telegram.org",
            "reactions.telegram.org",
            "claims.telegram.org",
            "x.telegram.org",
            "help.telegram.org",
            "docs.telegram.org",
            "schema.telegram.org",
            "dev.telegram.org",
            "contest.telegram.org",
            "premium.telegram.org",
            "settings.telegram.org",
            "qr.telegram.org",
            "stickers.telegram.org",
            "emoji.telegram.org",
            "themes.telegram.org",
            "donate.telegram.org",
            "fragment.telegram.org",
            "ton.telegram.org",
            "wallet.telegram.org",
            "pay.telegram.org",
            "telegram.me",
            "t.me",
            "telegram.dog",
            "telegra.ph",
            "telesco.pe",
            "web.telegram.me",
            "zws1.web.telegram.org",
            "zws2.web.telegram.org",
            "zws1.web.telegram.me",
            "zws2.web.telegram.me",
            "venus.web.telegram.org",
            "pluto.web.telegram.org",
            "aurora.web.telegram.org",
            "vesta.web.telegram.org",
            "voice.telegram.org",
            "cdn.telegram.org",
        ],
    ),
    (
        "Türkiye",
        &[
            "roblox.com",
            "wattpad.com",
            "pastebin.com",
            "4shared.com",
            "wikileaks.org",
            "bitly.com",
            "cutt.ly",
            "t2m.io",
        ],
    ),
    (
        "YouTube",
        &[
            "youtu.be",
            "youtube.com",
            "i.ytimg.com",
            "i9.ytimg.com",
            "yt3.ggpht.com",
            "yt4.ggpht.com",
            "googleapis.com",
            "jnn-pa.googleapis.com",
            "googleusercontent.com",
            "signaler-pa.youtube.com",
            "youtubei.googleapis.com",
            "manifest.googlevideo.com",
            "yt3.googleusercontent.com",
            "ytimg.com",
            "ggpht.com",
        ],
    ),
];

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CategoryRule {
    pub id: i64,
    pub rule_type: String,
    pub value: String,
    /// joins the DPI strategy test / endpoint deep probe
    pub test_enabled: bool,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TestSiteCategory {
    pub id: i64,
    pub name: String,
    /// dpi | proxy | direct — switched in Settings → Routing
    pub action: String,
    pub rules: Vec<CategoryRule>,
}

/// A category with its typed rules and its routing action — what
/// `routing.rs` builds the effective config from.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CategoryRules {
    pub id: i64,
    pub name: String,
    pub action: String,
    pub rules: Vec<CategoryRule>,
}

// ---------------------------------------------------------------------------
// Tauri commands (thin wrappers over the storage layer)
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn sites_list(state: State<AppState>) -> Result<Vec<TestSiteCategory>, String> {
    let conn = lock_db(&state)?;
    list_categories(&conn)
}

#[tauri::command]
pub fn sites_add_category(
    app: AppHandle,
    state: State<AppState>,
    name: String,
) -> Result<TestSiteCategory, String> {
    let name = validate_name(&name)?;
    let conn = lock_db(&state)?;
    let added = add_category(&conn, &name)?;
    let _ = app.emit(TEST_SITES_CHANGED_EVENT, ());
    // an empty category contributes no rules — the session stays as is
    Ok(added)
}

#[tauri::command]
pub fn sites_rename_category(
    app: AppHandle,
    state: State<AppState>,
    category_id: i64,
    name: String,
) -> Result<(), String> {
    let name = validate_name(&name)?;
    let conn = lock_db(&state)?;
    rename_category(&conn, category_id, &name)?;
    let _ = app.emit(TEST_SITES_CHANGED_EVENT, ());
    // names never reach the generated sing-box config
    Ok(())
}

#[tauri::command]
pub async fn sites_delete_category(
    app: AppHandle,
    state: State<'_, AppState>,
    category_id: i64,
) -> Result<(), String> {
    let had_rules = {
        let conn = lock_db(&state)?;
        let had_rules = category_rule_count(&conn, category_id)? > 0;
        delete_category(&conn, category_id)?;
        had_rules
    };
    let _ = app.emit(TEST_SITES_CHANGED_EVENT, ());
    if had_rules {
        crate::routing::apply_routing_change_live(&app).await;
    }
    Ok(())
}

/// Points a category at another outbound (dpi | proxy | direct): what its
/// rules do while the proxy is connected, and which test probes them.
/// Idempotent. A running session restarts with the new route (only an
/// empty category leaves it untouched — no rules, no config).
#[tauri::command]
pub async fn sites_set_action(
    app: AppHandle,
    state: State<'_, AppState>,
    category_id: i64,
    action: String,
) -> Result<(), String> {
    let action = validate_action(&action)?;
    let (changed, has_rules) = {
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
    let _ = app.emit(TEST_SITES_CHANGED_EVENT, ());
    if changed && has_rules {
        crate::routing::apply_routing_change_live(&app).await;
    }
    Ok(())
}

#[tauri::command]
pub async fn sites_add_rule(
    app: AppHandle,
    state: State<'_, AppState>,
    category_id: i64,
    rule_type: String,
    value: String,
) -> Result<CategoryRule, String> {
    let rule_type = validate_rule_type(&rule_type)?;
    let value = validate_rule_value(&rule_type, &value)?;
    let added = {
        let conn = lock_db(&state)?;
        add_rule(&conn, category_id, &rule_type, &value)?
    };
    let _ = app.emit(TEST_SITES_CHANGED_EVENT, ());
    crate::routing::apply_routing_change_live(&app).await;
    Ok(added)
}

#[tauri::command]
pub async fn sites_update_rule(
    app: AppHandle,
    state: State<'_, AppState>,
    rule_id: i64,
    rule_type: String,
    value: String,
) -> Result<(), String> {
    let rule_type = validate_rule_type(&rule_type)?;
    let value = validate_rule_value(&rule_type, &value)?;
    {
        let conn = lock_db(&state)?;
        update_rule(&conn, rule_id, &rule_type, &value)?;
    }
    let _ = app.emit(TEST_SITES_CHANGED_EVENT, ());
    crate::routing::apply_routing_change_live(&app).await;
    Ok(())
}

/// Toggles whether a rule joins the DPI strategy test and the endpoint
/// deep probe. Idempotent. Testing only — the session keeps its route.
#[tauri::command]
pub fn sites_set_rule_test(
    app: AppHandle,
    state: State<AppState>,
    rule_id: i64,
    test_enabled: bool,
) -> Result<(), String> {
    let conn = lock_db(&state)?;
    set_rule_test(&conn, rule_id, test_enabled)?;
    let _ = app.emit(TEST_SITES_CHANGED_EVENT, ());
    Ok(())
}

#[tauri::command]
pub async fn sites_delete_rule(
    app: AppHandle,
    state: State<'_, AppState>,
    rule_id: i64,
) -> Result<(), String> {
    {
        let conn = lock_db(&state)?;
        delete_rule(&conn, rule_id)?;
    }
    let _ = app.emit(TEST_SITES_CHANGED_EVENT, ());
    crate::routing::apply_routing_change_live(&app).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Storage layer
// ---------------------------------------------------------------------------

/// All categories with their rules, ordered by position then insertion.
pub fn list_categories(conn: &Connection) -> Result<Vec<TestSiteCategory>, String> {
    let mut stmt = conn
        .prepare("SELECT id, name, action FROM test_site_categories ORDER BY position, id")
        .map_err(db_err)?;
    let categories = stmt
        .query_map(
            [],
            |row| {
                Ok(TestSiteCategory {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    action: row.get(2)?,
                    rules: Vec::new(),
                })
            },
        )
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    drop(stmt);

    let rules = load_rules(conn)?;
    let mut categories = categories;
    for rule in rules {
        if let Some(category) = categories.iter_mut().find(|category| category.id == rule.0) {
            category.rules.push(rule.1);
        }
    }
    Ok(categories)
}

/// Every (category_id, rule) pair, in category-then-insertion order.
fn load_rules(conn: &Connection) -> Result<Vec<(i64, CategoryRule)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT s.id, s.category_id, s.rule_type, s.value, s.test_enabled
             FROM test_sites s
             JOIN test_site_categories c ON c.id = s.category_id
             ORDER BY c.position, c.id, s.id",
        )
        .map_err(db_err)?;
    let rules = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(1)?,
                CategoryRule {
                    id: row.get(0)?,
                    rule_type: row.get(2)?,
                    value: row.get(3)?,
                    test_enabled: row.get::<_, i64>(4)? != 0,
                },
            ))
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rules)
}

/// Every probe URL of the categories with the given action, flattened in
/// category order — only rules marked for testing that can be probed
/// (`url`, `domain_suffix`, `domain`). `dpi` feeds the strategy test,
/// `proxy` feeds the endpoint deep probe.
pub fn flat_test_rules(conn: &Connection, action: &str) -> Result<Vec<(i64, String)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT s.id, s.rule_type, s.value FROM test_sites s
             JOIN test_site_categories c ON c.id = s.category_id
             WHERE c.action = ?1 AND s.test_enabled = 1
             ORDER BY c.position, c.id, s.id",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![action], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, rule_type, value)| probe_url(&rule_type, &value).map(|url| (id, url)))
        .collect())
}

/// The URL a rule is probed with, when the rule type can be probed at all:
/// `url` rules are fetched as-is, bare-domain rules as `https://<value>/`.
/// Keyword/regex rules match too broadly to build a probe target and join
/// routing only.
pub fn probe_url(rule_type: &str, value: &str) -> Option<String> {
    match rule_type {
        RULE_TYPE_URL => Some(value.to_string()),
        RULE_TYPE_DOMAIN_SUFFIX | RULE_TYPE_DOMAIN => Some(format!("https://{value}/")),
        _ => None,
    }
}

/// Every category with its typed rules and routing action — the routing
/// contribution of the catalog. Junk rule types (hand-edited DBs) ride
/// along and are skipped by the routing loader, not here.
pub fn category_rules(conn: &Connection) -> Result<Vec<CategoryRules>, String> {
    Ok(list_categories(conn)?
        .into_iter()
        .map(|category| CategoryRules {
            id: category.id,
            name: category.name,
            action: category.action,
            rules: category.rules,
        })
        .collect())
}

fn add_category(conn: &Connection, name: &str) -> Result<TestSiteCategory, String> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM test_site_categories", [], |row| row.get(0))
        .map_err(db_err)?;
    if count as usize >= MAX_CATEGORIES {
        return Err(format!("at most {MAX_CATEGORIES} categories can be configured"));
    }
    if category_named(conn, name)?.is_some() {
        return Err(format!("category {name:?} already exists"));
    }
    conn.execute(
        "INSERT INTO test_site_categories (name, position)
         VALUES (?1, (SELECT COALESCE(MAX(position), -1) + 1 FROM test_site_categories))",
        params![name],
    )
    .map_err(db_err)?;
    Ok(TestSiteCategory {
        id: conn.last_insert_rowid(),
        name: name.to_string(),
        action: ACTION_DPI.to_string(),
        rules: Vec::new(),
    })
}

fn rename_category(conn: &Connection, category_id: i64, name: &str) -> Result<(), String> {
    if let Some(existing) = category_named(conn, name)? {
        if existing != category_id {
            return Err(format!("category {name:?} already exists"));
        }
    }
    let updated = conn
        .execute(
            "UPDATE test_site_categories SET name = ?2 WHERE id = ?1",
            params![category_id, name],
        )
        .map_err(db_err)?;
    if updated == 0 {
        return Err(format!("category {category_id} not found"));
    }
    Ok(())
}

pub(crate) fn set_action(conn: &Connection, category_id: i64, action: &str) -> Result<(), String> {
    let updated = conn
        .execute(
            "UPDATE test_site_categories SET action = ?2 WHERE id = ?1",
            params![category_id, action],
        )
        .map_err(db_err)?;
    if updated == 0 {
        return Err(format!("category {category_id} not found"));
    }
    Ok(())
}

fn delete_category(conn: &Connection, category_id: i64) -> Result<(), String> {
    // rules and their results go away via the FK cascades
    conn.execute("DELETE FROM test_site_categories WHERE id = ?1", params![category_id])
        .map_err(db_err)?;
    Ok(())
}

fn category_named(conn: &Connection, name: &str) -> Result<Option<i64>, String> {
    conn.query_row(
        "SELECT id FROM test_site_categories WHERE name = ?1",
        params![name],
        |row| row.get(0),
    )
    .map(Some)
    .or_else(|error| match error {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(db_err(other)),
    })
}

/// Whether a category contributes rules to the generated route — an empty
/// category's mutations never reach the sing-box config.
fn category_rule_count(conn: &Connection, category_id: i64) -> Result<i64, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM test_sites WHERE category_id = ?1",
        params![category_id],
        |row| row.get(0),
    )
    .map_err(db_err)
}

fn add_rule(conn: &Connection, category_id: i64, rule_type: &str, value: &str) -> Result<CategoryRule, String> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM test_site_categories WHERE id = ?1)",
            params![category_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if !exists {
        return Err(format!("category {category_id} not found"));
    }
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM test_sites WHERE category_id = ?1",
            params![category_id],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if count as usize >= MAX_RULES_PER_CATEGORY {
        return Err(format!(
            "at most {MAX_RULES_PER_CATEGORY} rules can be stored in one category"
        ));
    }
    let duplicate: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM test_sites WHERE category_id = ?1 AND rule_type = ?2 AND value = ?3)",
            params![category_id, rule_type, value],
            |row| row.get(0),
        )
        .map_err(db_err)?;
    if duplicate {
        return Err("this rule is already in the category".into());
    }
    conn.execute(
        "INSERT INTO test_sites (category_id, rule_type, value) VALUES (?1, ?2, ?3)",
        params![category_id, rule_type, value],
    )
    .map_err(db_err)?;
    Ok(CategoryRule {
        id: conn.last_insert_rowid(),
        rule_type: rule_type.to_string(),
        value: value.to_string(),
        test_enabled: true,
    })
}

fn update_rule(conn: &Connection, rule_id: i64, rule_type: &str, value: &str) -> Result<(), String> {
    let updated = conn
        .execute(
            "UPDATE test_sites SET rule_type = ?2, value = ?3 WHERE id = ?1",
            params![rule_id, rule_type, value],
        )
        .map_err(|error| match error {
            // the UNIQUE(category_id, rule_type, value) constraint
            rusqlite::Error::SqliteFailure(failure, _)
                if failure.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                "this rule is already in the category".to_string()
            }
            other => db_err(other),
        })?;
    if updated == 0 {
        return Err(format!("rule {rule_id} not found"));
    }
    Ok(())
}

fn set_rule_test(conn: &Connection, rule_id: i64, test_enabled: bool) -> Result<(), String> {
    let updated = conn
        .execute(
            "UPDATE test_sites SET test_enabled = ?2 WHERE id = ?1",
            params![rule_id, test_enabled as i64],
        )
        .map_err(db_err)?;
    if updated == 0 {
        return Err(format!("rule {rule_id} not found"));
    }
    Ok(())
}

fn delete_rule(conn: &Connection, rule_id: i64) -> Result<(), String> {
    // strategy results for this rule go away via the FK cascade
    conn.execute("DELETE FROM test_sites WHERE id = ?1", params![rule_id])
        .map_err(db_err)?;
    Ok(())
}

/// Inserts the default catalog once — only into a completely empty table
/// (any user edits, even a single added category, block re-seeding). Called
/// from app setup, not from `db::open`, so unit tests start clean.
pub fn seed_default_sites(conn: &Connection) -> Result<(), String> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM test_site_categories", [], |row| row.get(0))
        .map_err(db_err)?;
    if count > 0 {
        return Ok(());
    }
    for (position, (name, hosts)) in DEFAULT_SITES.iter().enumerate() {
        conn.execute(
            "INSERT INTO test_site_categories (name, position) VALUES (?1, ?2)",
            params![name, position as i64],
        )
        .map_err(db_err)?;
        let category_id = conn.last_insert_rowid();
        for host in *hosts {
            let value = validate_rule_value(RULE_TYPE_DOMAIN_SUFFIX, host)?;
            conn.execute(
                "INSERT INTO test_sites (category_id, rule_type, value) VALUES (?1, ?2, ?3)",
                params![category_id, RULE_TYPE_DOMAIN_SUFFIX, value],
            )
            .map_err(db_err)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

pub(crate) fn validate_rule_type(value: &str) -> Result<String, String> {
    if RULE_TYPES.contains(&value) {
        Ok(value.to_string())
    } else {
        Err(format!("unknown rule type `{value}` (expected one of: {RULE_TYPES:?})"))
    }
}

/// Validates and normalizes a rule value for its type. Returns the form
/// used for storage and deduplication.
pub(crate) fn validate_rule_value(rule_type: &str, raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("the rule value must not be empty".into());
    }
    if trimmed.chars().any(char::is_control) {
        return Err("the rule value must not contain control characters".into());
    }
    match rule_type {
        RULE_TYPE_URL => validate_test_url(trimmed),
        RULE_TYPE_DOMAIN_SUFFIX | RULE_TYPE_DOMAIN => {
            let lowered = trimmed.to_lowercase();
            if lowered.len() > 253 {
                return Err("the domain is too long (max 253 characters)".into());
            }
            if lowered
                .chars()
                .any(|c| matches!(c, '/' | ':' | '?' | '#' | '@' | ' ' | '\t'))
            {
                return Err(
                    "a domain rule must be a bare host like `example.com` — for full URLs use the URL type".into(),
                );
            }
            Ok(lowered)
        }
        RULE_TYPE_KEYWORD | RULE_TYPE_REGEX => {
            if trimmed.len() > 256 {
                return Err("the pattern is too long (max 256 characters)".into());
            }
            Ok(trimmed.to_string())
        }
        other => Err(format!("unknown rule type `{other}`")),
    }
}

/// The hostname part of a stored `url`-typed rule — what it routes by.
pub(crate) fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
        .unwrap_or_else(|| url.trim().to_string())
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
    use super::*;

    fn test_db() -> Connection {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "megathrone-sites-{}-{id}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        crate::db::open(&path).expect("test db should open")
    }

    #[test]
    fn default_catalog_seeds_once_and_lists_grouped() {
        let conn = test_db();
        seed_default_sites(&conn).expect("seed");

        let categories = list_categories(&conn).expect("list");
        assert_eq!(categories.len(), DEFAULT_SITES.len());
        assert_eq!(categories[0].name, "General");
        assert_eq!(categories[0].rules.len(), DEFAULT_SITES[0].1.len());
        assert!(
            categories.iter().all(|category| category.action == "dpi"),
            "seeded as dpi-routed"
        );
        // hosts are stored as whole-domain suffix rules, marked for testing
        assert_eq!(categories[0].rules[0].rule_type, RULE_TYPE_DOMAIN_SUFFIX);
        assert_eq!(categories[0].rules[0].value, "rutracker.org");
        assert!(categories[0].rules.iter().all(|rule| rule.test_enabled));
        // every rule is unique
        let total: usize = categories.iter().map(|category| category.rules.len()).sum();
        let flat = flat_test_rules(&conn, "dpi").expect("flat");
        assert_eq!(flat.len(), total);
        assert_eq!(flat[0].1, "https://rutracker.org/");

        // re-seeding into a non-empty table is a no-op
        seed_default_sites(&conn).expect("seed again");
        assert_eq!(list_categories(&conn).expect("list").len(), DEFAULT_SITES.len());
    }

    #[test]
    fn actions_split_rules_between_the_tests_and_routing() {
        let conn = test_db();
        let category = add_category(&conn, "Cat").expect("add");
        assert_eq!(category.action, "dpi");
        add_rule(&conn, category.id, RULE_TYPE_URL, "https://a.example/x").expect("rule");
        add_rule(&conn, category.id, RULE_TYPE_DOMAIN_SUFFIX, "b.example").expect("rule");
        add_rule(&conn, category.id, RULE_TYPE_KEYWORD, "blocked").expect("rule");

        // keyword rules route but never join a test
        assert_eq!(flat_test_rules(&conn, "dpi").expect("flat").len(), 2);
        assert!(flat_test_rules(&conn, "proxy").expect("flat").is_empty());
        let hosts = category_rules(&conn).expect("cats");
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "Cat");
        assert_eq!(hosts[0].action, "dpi");
        assert_eq!(hosts[0].rules.len(), 3);

        // direct: neither test probes it, routing still knows the action
        set_action(&conn, category.id, "direct").expect("direct");
        assert!(flat_test_rules(&conn, "dpi").expect("flat").is_empty(), "not probed anymore");
        assert_eq!(category_rules(&conn).expect("cats")[0].action, "direct");
        // …but the category stays listed with its rules
        let listed = list_categories(&conn).expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].action, "direct");
        assert_eq!(listed[0].rules.len(), 3);

        // proxy: joins the endpoint deep probe instead
        set_action(&conn, category.id, "proxy").expect("proxy");
        assert_eq!(flat_test_rules(&conn, "proxy").expect("flat").len(), 2);
        assert!(flat_test_rules(&conn, "dpi").expect("flat").is_empty());

        assert!(set_action(&conn, 999, "proxy").is_err());
    }

    #[test]
    fn rule_test_flag_and_probe_targets() {
        let conn = test_db();
        let category = add_category(&conn, "Cat").expect("add");
        let url_rule = add_rule(&conn, category.id, RULE_TYPE_URL, "https://a.example/x")
            .expect("rule");
        let suffix = add_rule(&conn, category.id, RULE_TYPE_DOMAIN_SUFFIX, "b.example")
            .expect("rule");
        let keyword = add_rule(&conn, category.id, RULE_TYPE_KEYWORD, "key").expect("rule");

        assert_eq!(
            probe_url(&url_rule.rule_type, &url_rule.value).as_deref(),
            Some("https://a.example/x")
        );
        assert_eq!(
            probe_url(&suffix.rule_type, &suffix.value).as_deref(),
            Some("https://b.example/")
        );
        assert!(probe_url(&keyword.rule_type, &keyword.value).is_none());

        // opting a rule out of testing removes it from the probe list only
        set_rule_test(&conn, url_rule.id, false).expect("toggle");
        let flat = flat_test_rules(&conn, "dpi").expect("flat");
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].0, suffix.id);
        let listed = list_categories(&conn).expect("list");
        assert!(!listed[0].rules[0].test_enabled, "the stored flag follows");
        assert!(listed[0].rules.iter().skip(1).all(|rule| rule.test_enabled));
        assert!(set_rule_test(&conn, 999, true).is_err());
    }

    #[test]
    fn category_crud_and_duplicate_names() {
        let conn = test_db();

        let first = add_category(&conn, "Blocked").expect("add");
        let second = add_category(&conn, "News").expect("add");
        assert!(add_category(&conn, "Blocked").is_err(), "duplicate names are rejected");
        assert_eq!(list_categories(&conn).expect("list").len(), 2);

        // renaming onto another category's name is a conflict, onto itself is fine
        assert!(rename_category(&conn, first.id, "News").is_err());
        rename_category(&conn, first.id, "blocked").expect("self rename");
        assert!(rename_category(&conn, 999, "X").is_err());

        // deleting removes the category and nothing else
        delete_category(&conn, second.id).expect("delete");
        let categories = list_categories(&conn).expect("list");
        assert_eq!(categories.len(), 1);
        assert_eq!(categories[0].name, "blocked");
        assert!(delete_category(&conn, second.id).is_ok(), "idempotent");
    }

    #[test]
    fn rule_crud_dedup_and_cascades() {
        let conn = test_db();
        let category = add_category(&conn, "Cat").expect("add");

        // the command layer validates before storing — mirror that here
        let normalized = validate_rule_value(RULE_TYPE_DOMAIN_SUFFIX, "YouTube.com").expect("valid");
        let rule = add_rule(&conn, category.id, RULE_TYPE_DOMAIN_SUFFIX, &normalized).expect("add");
        assert_eq!(rule.value, "youtube.com", "domains are lowercased");
        assert!(
            add_rule(&conn, category.id, RULE_TYPE_DOMAIN_SUFFIX, "youtube.com").is_err(),
            "normalized duplicate"
        );
        // the same value under another type is its own rule
        add_rule(&conn, category.id, RULE_TYPE_DOMAIN, "youtube.com").expect("same value, other type");
        assert!(add_rule(&conn, 999, RULE_TYPE_DOMAIN_SUFFIX, "x.example").is_err(), "unknown category");

        update_rule(&conn, rule.id, RULE_TYPE_URL, "https://changed.example/path").expect("update");
        assert!(
            validate_rule_value(RULE_TYPE_DOMAIN_SUFFIX, "https://x.example/").is_err(),
            "a full URL is rejected for domain rules"
        );
        assert!(validate_rule_value(RULE_TYPE_KEYWORD, "  ").is_err());
        assert!(validate_rule_type("regexp").is_err());
        assert!(update_rule(&conn, 999, RULE_TYPE_URL, "https://x.example/").is_err());

        // deleting the rule cascades its strategy results
        conn.execute(
            "INSERT INTO dpi_strategies (name, args) VALUES ('S', '-s2')",
            [],
        )
        .expect("seed strategy");
        let strategy_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO dpi_url_results (strategy_id, url_id, ok) VALUES (?1, ?2, 1)",
            params![strategy_id, rule.id],
        )
        .expect("seed result");
        delete_rule(&conn, rule.id).expect("delete");
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM dpi_url_results", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 0);
        assert_eq!(flat_test_rules(&conn, "dpi").expect("flat").len(), 1, "the domain rule stays");
    }

    #[test]
    fn caps_are_enforced() {
        let conn = test_db();
        for index in 0..MAX_CATEGORIES {
            add_category(&conn, &format!("C{index}")).expect("add below cap");
        }
        let error = add_category(&conn, "One too many").expect_err("cap");
        assert!(error.contains("at most"));

        // per-category rule cap on a fresh database
        let conn = test_db();
        let category = add_category(&conn, "Cat").expect("add");
        for index in 0..MAX_RULES_PER_CATEGORY {
            add_rule(&conn, category.id, RULE_TYPE_DOMAIN_SUFFIX, &format!("host{index}.example"))
                .expect("add");
        }
        let error =
            add_rule(&conn, category.id, RULE_TYPE_DOMAIN_SUFFIX, "one-too-many.example")
                .expect_err("cap");
        assert!(error.contains("at most"));
    }
}
