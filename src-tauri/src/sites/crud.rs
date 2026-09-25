use rusqlite::{Connection, params};

use super::model::{CategoryRule, TestSiteCategory};
use super::queries::db_err;
use crate::routing::ACTION_DPI;

pub const MAX_CATEGORIES: usize = 24;
pub const MAX_RULES_PER_CATEGORY: usize = 128;

pub(super) fn add_category(conn: &Connection, name: &str) -> Result<TestSiteCategory, String> {
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

pub(super) fn rename_category(conn: &Connection, category_id: i64, name: &str) -> Result<(), String> {
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

pub(super) fn delete_category(conn: &Connection, category_id: i64) -> Result<(), String> {
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
pub(super) fn category_rule_count(conn: &Connection, category_id: i64) -> Result<i64, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM test_sites WHERE category_id = ?1",
        params![category_id],
        |row| row.get(0),
    )
    .map_err(db_err)
}

pub(super) fn add_rule(conn: &Connection, category_id: i64, rule_type: &str, value: &str) -> Result<CategoryRule, String> {
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

pub(super) fn update_rule(conn: &Connection, rule_id: i64, rule_type: &str, value: &str) -> Result<(), String> {
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

pub(super) fn set_rule_test(conn: &Connection, rule_id: i64, test_enabled: bool) -> Result<(), String> {
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

pub(super) fn delete_rule(conn: &Connection, rule_id: i64) -> Result<(), String> {
    // strategy results for this rule go away via the FK cascade
    conn.execute("DELETE FROM test_sites WHERE id = ?1", params![rule_id])
        .map_err(db_err)?;
    Ok(())
}

// Tests of the private CRUD helpers live here, next to the definitions —
// they are unreachable from any other file.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sites::sites_tests::test_db;
    use crate::sites::{
        flat_test_rules, list_categories, validate_rule_type, validate_rule_value,
        RULE_TYPE_DOMAIN, RULE_TYPE_DOMAIN_SUFFIX, RULE_TYPE_KEYWORD, RULE_TYPE_URL,
    };

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
