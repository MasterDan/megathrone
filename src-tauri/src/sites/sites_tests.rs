use rusqlite::Connection;

use super::catalog::DEFAULT_SITES;
use super::crud::{add_category, add_rule, set_rule_test};
use super::*;

pub(super) fn test_db() -> Connection {
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "megathrone-sites-{}-{id}.db",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    crate::db::open(&path, &crate::db::Migrations::embedded()).expect("test db should open")
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
