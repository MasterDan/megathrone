use rusqlite::{Connection, params};

use super::model::{CategoryRule, CategoryRules, TestSiteCategory};
use super::validation::probe_url;

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

pub(super) fn db_err(error: rusqlite::Error) -> String {
    format!("database error: {error}")
}
