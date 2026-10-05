use rusqlite::{params, Connection};

use super::model::{EndpointItem, EndpointUrlStatus, ItemDetail};
use super::storage::db_err;

/// Columns shared by every EndpointItem query: base endpoint fields plus the
/// deep-probe score over the *proxy*-routed category rules (`url_ok` of the
/// probed `url_total`; 0/0 — this endpoint was not deep-probed last scan).
/// Only rules marked for testing are ever probed, so opted-out rules count
/// on neither side.
pub(super) const URL_SCORE_COLUMNS: &str = "(
        SELECT COUNT(*) FROM endpoint_url_results r
        JOIN test_sites s ON s.id = r.url_id
        JOIN test_site_categories c ON c.id = s.category_id
        WHERE r.endpoint_id = e.id AND r.available = 1 AND c.action = 'proxy'
     ) AS url_ok, (
        SELECT COUNT(*) FROM endpoint_url_results r
        JOIN test_sites s ON s.id = r.url_id
        JOIN test_site_categories c ON c.id = s.category_id
        WHERE r.endpoint_id = e.id AND c.action = 'proxy'
     ) AS url_total";

/// Canonical endpoint order of the endpoints grid — `list_items` pages it
/// and `item_index` resolves positions in it, so both must stay in sync.
const CANONICAL_ORDER: &str = "CASE e.available WHEN 1 THEN 0 WHEN 0 THEN 2 ELSE 1 END,
                  e.latency_ms,
                  url_ok DESC,
                  e.order_index";

pub(super) fn list_items(conn: &Connection, profile_id: i64, offset: i64, limit: i64) -> Result<Vec<EndpointItem>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT e.id, e.tag, e.protocol, e.server, e.server_port, e.available, e.latency_ms,
                    e.up_bytes, e.down_bytes, e.speed_bps,
                    {URL_SCORE_COLUMNS}
             FROM endpoints e
             WHERE e.profile_id = ?1
             ORDER BY {CANONICAL_ORDER}
             LIMIT ?2 OFFSET ?3"
        ))
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![profile_id, limit.clamp(1, 500), offset], row_to_item)
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rows)
}

/// 0-based index of an endpoint in the grid's canonical order; None when the
/// endpoint is gone or belongs to another profile.
pub(super) fn item_index(conn: &Connection, profile_id: i64, item_id: i64) -> Result<Option<i64>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT e.id, {URL_SCORE_COLUMNS}
             FROM endpoints e
             WHERE e.profile_id = ?1
             ORDER BY {CANONICAL_ORDER}"
        ))
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![profile_id], |row| row.get::<_, i64>(0))
        .map_err(db_err)?;
    for (index, row) in rows.enumerate() {
        if row.map_err(db_err)? == item_id {
            return Ok(Some(index as i64));
        }
    }
    Ok(None)
}

/// Latest outcome of every deep-probed proxy-category rule for one
/// endpoint, grouped in category order (shown as the URL it probed).
pub(super) fn endpoint_url_statuses(conn: &Connection, item_id: i64) -> Result<Vec<EndpointUrlStatus>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT u.id, c.name, u.rule_type, u.value, r.available, r.latency_ms
             FROM endpoint_url_results r
             JOIN test_sites u ON u.id = r.url_id
             JOIN test_site_categories c ON c.id = u.category_id
             WHERE r.endpoint_id = ?1 AND c.action = 'proxy'
             ORDER BY c.position, c.id, u.id",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![item_id], |row| {
            let rule_type: String = row.get(2)?;
            let value: String = row.get(3)?;
            Ok(EndpointUrlStatus {
                url_id: row.get(0)?,
                category: row.get(1)?,
                url: crate::sites::probe_url(&rule_type, &value).unwrap_or(value),
                available: row.get::<_, Option<i64>>(4)?.map(|value| value != 0),
                latency_ms: row.get(5)?,
            })
        })
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    Ok(rows)
}

pub(super) fn item_detail(conn: &Connection, item_id: i64) -> Result<ItemDetail, String> {
    conn.query_row(
        "SELECT raw, outbound_json FROM endpoints WHERE id = ?1",
        params![item_id],
        |row| Ok(ItemDetail { raw: row.get(0)?, outbound_json: row.get(1)? }),
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => format!("endpoint {item_id} not found"),
        other => db_err(other),
    })
}

/// The selected endpoint is remembered as its raw link: endpoint ids are not
/// stable across content updates, but the link usually is. Returns None when
/// the stored choice no longer matches anything (the endpoint disappeared).
pub(super) fn get_selected_endpoint(conn: &Connection, profile_id: i64) -> Result<Option<EndpointItem>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT e.id, e.tag, e.protocol, e.server, e.server_port, e.available, e.latency_ms,
                    e.up_bytes, e.down_bytes, e.speed_bps,
                    {URL_SCORE_COLUMNS}
             FROM profiles p
             JOIN endpoints e ON e.profile_id = p.id AND e.raw = p.selected_endpoint_key
             WHERE p.id = ?1
             ORDER BY e.order_index
             LIMIT 1"
        ))
        .map_err(db_err)?;
    let mut rows = stmt
        .query_map(params![profile_id], row_to_item)
        .map_err(db_err)?;
    match rows.next() {
        Some(row) => Ok(Some(row.map_err(db_err)?)),
        None => Ok(None),
    }
}

/// Stores the selection (endpoint id → its raw link). Returns whether the
/// stored key actually changed — callers use it to skip no-op restarts.
pub(crate) fn set_selected_endpoint(
    conn: &Connection,
    profile_id: i64,
    item_id: Option<i64>,
) -> Result<bool, String> {
    let key = match item_id {
        Some(item_id) => Some(conn
            .query_row(
                "SELECT raw FROM endpoints WHERE id = ?1 AND profile_id = ?2",
                params![item_id, profile_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    format!("endpoint {item_id} not found in profile {profile_id}")
                }
                other => db_err(other),
            })?),
        None => None,
    };
    let previous: Option<String> = conn
        .query_row(
            "SELECT selected_endpoint_key FROM profiles WHERE id = ?1",
            params![profile_id],
            |row| row.get(0),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => format!("profile {profile_id} not found"),
            other => db_err(other),
        })?;
    if previous == key {
        return Ok(false);
    }
    let affected = conn
        .execute(
            "UPDATE profiles SET selected_endpoint_key = ?2, updated_at = datetime('now')
             WHERE id = ?1",
            params![profile_id, key],
        )
        .map_err(db_err)?;
    if affected == 0 {
        return Err(format!("profile {profile_id} not found"));
    }
    Ok(true)
}

fn row_to_item(row: &rusqlite::Row) -> rusqlite::Result<EndpointItem> {
    Ok(EndpointItem {
        id: row.get(0)?,
        tag: row.get(1)?,
        protocol: row.get(2)?,
        server: row.get(3)?,
        server_port: row.get(4)?,
        available: row.get::<_, Option<i64>>(5)?.map(|v| v != 0),
        latency_ms: row.get(6)?,
        up_bytes: row.get(7)?,
        down_bytes: row.get(8)?,
        speed_bps: row.get(9)?,
        url_ok: row.get(10)?,
        url_total: row.get(11)?,
    })
}
