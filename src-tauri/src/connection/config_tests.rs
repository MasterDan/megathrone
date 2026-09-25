//! Tests of the generated sing-box run config: baking, the routing rules,
//! the raw local proxy port, the DPI-only degradation and the port
//! fallback.

use serde_json::{Value, json};

use crate::dpi;
use crate::latency;
use crate::routing;
use crate::sites;

use super::config::{
    RAW_IN_TAG, bake_endpoints, build_run_config, dpi_only_allowed, resolve_raw_port,
    BakedEndpoint,
};
use super::state::{DIRECT_TAG, DPI_TAG, MODE_OFF, MODE_TUN, PROXY_TAG};

#[test]
fn dpi_only_needs_a_strategy_and_dpi_routing() {
    // routing with no dpi categories and a proxy fallback
    let no_dpi = routing::RoutingConfig {
        categories: vec![routing::RouteCategory {
            id: 1,
            name: "Direct Cat".to_string(),
            action: routing::ACTION_DIRECT.to_string(),
            rules: vec![routing::RouteRule {
                rule_type: sites::RULE_TYPE_DOMAIN.to_string(),
                value: "example.org".to_string(),
            }],
        }],
        fallback: routing::ACTION_PROXY.to_string(),
    };
    assert!(!dpi_only_allowed(None, &no_dpi));

    let strategy = dpi::DpiStrategy {
        id: 1,
        name: "S".to_string(),
        args: String::new(),
        is_active: true,
        url_ok: 0,
        url_total: 0,
        tested: 0,
    };
    // strategy but no dpi routing → still refused
    assert!(!dpi_only_allowed(Some(&strategy), &no_dpi));

    // a dpi category or a dpi fallback makes it viable
    let mut dpi_category = no_dpi.clone();
    dpi_category.categories[0].action = routing::ACTION_DPI.to_string();
    assert!(dpi_only_allowed(Some(&strategy), &dpi_category));

    let dpi_fallback =
        routing::RoutingConfig { fallback: routing::ACTION_DPI.to_string(), ..no_dpi.clone() };
    assert!(dpi_only_allowed(Some(&strategy), &dpi_fallback));
}

/// One baked endpoint per outbound value, tags `mt-1`…`mt-N` by position.
fn baked(outbounds: &[Value]) -> Vec<BakedEndpoint> {
    outbounds
        .iter()
        .enumerate()
        .map(|(index, outbound)| BakedEndpoint {
            id: index as i64 + 1,
            tag: format!("mt-{}", index + 1),
            outbound: outbound.clone(),
        })
        .collect()
}

#[test]
fn bake_endpoints_assigns_sequential_unique_tags() {
    let rows = vec![
        (7, json!({ "type": "vless", "server": "1.1.1.1", "server_port": 443 })),
        (9, json!({ "type": "trojan", "server": "2.2.2.2", "server_port": 443 })),
    ];
    let baked = bake_endpoints(rows);
    assert_eq!(baked[0].id, 7);
    assert_eq!(baked[0].tag, "mt-1");
    assert_eq!(baked[1].id, 9);
    assert_eq!(baked[1].tag, "mt-2");
}

#[test]
fn build_run_config_wires_mixed_and_direct_rules() {
    let endpoints = baked(&[json!({
        "type": "vless", "tag": "original #2", "server": "1.2.3.4", "server_port": 443
    })]);

    let config = build_run_config(&endpoints, Some("mt-1"), MODE_OFF, 7897, 45678, None, None);

    assert_eq!(config["inbounds"].as_array().expect("inbounds").len(), 1);
    assert_eq!(config["inbounds"][0]["type"], "mixed");
    assert_eq!(config["inbounds"][0]["listen_port"], 7897);
    // the endpoint keeps its synthetic tag; the selector rides on top
    // of it as the outbound the routing rules reference
    let outbounds = config["outbounds"].as_array().expect("outbounds");
    assert_eq!(outbounds.len(), 3);
    assert_eq!(outbounds[0]["tag"], "mt-1", "source tags are replaced");
    assert_eq!(outbounds[0]["type"], "vless");
    assert_eq!(outbounds[1]["type"], "selector");
    assert_eq!(outbounds[1]["tag"], PROXY_TAG);
    assert_eq!(outbounds[1]["default"], "mt-1");
    assert_eq!(outbounds[1]["outbounds"], json!(["mt-1"]));
    assert_eq!(outbounds[1]["interrupt_exist_connections"], false);
    assert_eq!(outbounds[2]["type"], "direct");
    assert_eq!(config["route"]["final"], PROXY_TAG);
    assert_eq!(config["route"]["rules"].as_array().expect("rules").len(), 1);
    assert_eq!(config["route"]["rules"][0]["outbound"], DIRECT_TAG);
    assert_eq!(
        config["experimental"]["clash_api"]["external_controller"],
        "127.0.0.1:45678"
    );
}

/// The selector lists every baked endpoint — any of them is a
/// restart-free switch target — and the default may be any of them.
#[test]
fn build_run_config_selector_lists_every_baked_endpoint() {
    let endpoints = baked(&[
        json!({ "type": "vless", "server": "1.2.3.4", "server_port": 443 }),
        json!({ "type": "trojan", "server": "5.6.7.8", "server_port": 443 }),
    ]);

    let config = build_run_config(&endpoints, Some("mt-2"), MODE_OFF, 7897, 45678, None, None);

    let outbounds = config["outbounds"].as_array().expect("outbounds");
    let selector = &outbounds[2];
    assert_eq!(selector["type"], "selector");
    assert_eq!(selector["outbounds"], json!(["mt-1", "mt-2"]));
    assert_eq!(selector["default"], "mt-2");
}

#[test]
fn build_run_config_tun_adds_the_tun_inbound() {
    let endpoints = baked(&[json!({
        "type": "trojan", "server": "1.2.3.4", "server_port": 443
    })]);
    let config =
        build_run_config(&endpoints, Some("mt-1"), MODE_TUN, 7897, 45678, None, None);
    let inbounds = config["inbounds"].as_array().expect("inbounds");
    assert_eq!(inbounds.len(), 2);
    assert_eq!(inbounds[0]["type"], "mixed");
    assert_eq!(inbounds[1]["type"], "tun");
    assert_eq!(inbounds[1]["auto_route"], true);
}

#[test]
fn build_run_config_sanitizes_junk_fingerprints() {
    let poisoned = json!({
        "type": "vless", "server": "1.2.3.4", "server_port": 443,
        "tls": { "enabled": true, "utls": { "enabled": true, "fingerprint": "unsafe" } }
    });

    let config = build_run_config(&baked(&[poisoned]), Some("mt-1"), MODE_OFF, 7897, 45678, None, None);

    let utls = &config["outbounds"][0]["tls"]["utls"];
    assert!(utls.get("fingerprint").is_none(), "junk fingerprint must be dropped");
    assert_eq!(utls["enabled"], true);
}

/// A routing fixture with one category per action, in this order — the
/// proxy category mixes every rule kind to cover the bucketing.
fn routing_fixture(fallback: &str) -> routing::RoutingConfig {
    let rule = |rule_type: &str, value: &str| routing::RouteRule {
        rule_type: rule_type.to_string(),
        value: value.to_string(),
    };
    routing::RoutingConfig {
        categories: vec![
            routing::RouteCategory {
                id: 1,
                name: "DPI Cat".to_string(),
                action: routing::ACTION_DPI.to_string(),
                rules: vec![
                    rule(sites::RULE_TYPE_DOMAIN_SUFFIX, "youtube.com"),
                    rule(sites::RULE_TYPE_DOMAIN_SUFFIX, "youtu.be"),
                ],
            },
            routing::RouteCategory {
                id: 2,
                name: "Direct Cat".to_string(),
                action: routing::ACTION_DIRECT.to_string(),
                rules: vec![rule(sites::RULE_TYPE_DOMAIN, "example.org")],
            },
            routing::RouteCategory {
                id: 3,
                name: "Proxy Cat".to_string(),
                action: routing::ACTION_PROXY.to_string(),
                rules: vec![
                    rule(sites::RULE_TYPE_DOMAIN_SUFFIX, "speedtest.net"),
                    rule(sites::RULE_TYPE_URL, "https://media.example/clip"),
                    rule(sites::RULE_TYPE_KEYWORD, "blocked"),
                    rule(sites::RULE_TYPE_REGEX, r"(^|\.)vk\.com$"),
                ],
            },
        ],
        fallback: fallback.to_string(),
    }
}

#[test]
fn build_run_config_routes_categories_by_action() {
    let fixture = routing_fixture("proxy");
    let endpoints = baked(&[json!({ "type": "vless", "server": "1.2.3.4", "server_port": 443 })]);

    // with the tunnel up: private → direct, sniff, then one rule per
    // (category, matcher kind) in category order — `url` rules ride
    // the suffix bucket by their hostname
    let config = build_run_config(
        &endpoints,
        Some("mt-1"),
        MODE_OFF,
        7897,
        45678,
        Some((&fixture, Some(1080))),
        None,
    );
    let rules = config["route"]["rules"].as_array().expect("rules");
    assert_eq!(rules.len(), 7);
    assert_eq!(rules[0]["outbound"], DIRECT_TAG);
    assert_eq!(rules[0]["ip_is_private"], true);
    assert_eq!(rules[1]["action"], "sniff");
    assert_eq!(rules[2]["domain_suffix"], json!(["youtube.com", "youtu.be"]));
    assert_eq!(rules[2]["outbound"], DPI_TAG);
    assert_eq!(rules[3]["domain"], json!(["example.org"]));
    assert_eq!(rules[3]["outbound"], DIRECT_TAG);
    assert_eq!(rules[4]["domain_suffix"], json!(["speedtest.net", "media.example"]));
    assert_eq!(rules[4]["outbound"], PROXY_TAG);
    assert_eq!(rules[5]["domain_keyword"], json!(["blocked"]));
    assert_eq!(rules[5]["outbound"], PROXY_TAG);
    assert_eq!(rules[6]["domain_regex"], json!([r"(^|\.)vk\.com$"]));
    assert_eq!(rules[6]["outbound"], PROXY_TAG);
    assert_eq!(config["route"]["final"], PROXY_TAG);

    // without a running tunnel the dpi category rule disappears; the
    // other domain rules still sniff
    let config = build_run_config(
        &endpoints,
        Some("mt-1"),
        MODE_OFF,
        7897,
        45678,
        Some((&fixture, None)),
        None,
    );
    let rules = config["route"]["rules"].as_array().expect("rules");
    assert!(
        !rules.iter().any(|rule| rule["outbound"] == DPI_TAG),
        "dpi rules vanish without the tunnel"
    );
    assert!(
        rules.iter().any(|rule| rule.get("action") == Some(&json!("sniff"))),
        "the surviving domain rules still need the hostname"
    );

    // categories without rules contribute nothing
    let mut empty = routing_fixture("proxy");
    empty.categories.clear();
    let config = build_run_config(
        &endpoints,
        Some("mt-1"),
        MODE_OFF,
        7897,
        45678,
        Some((&empty, Some(1080))),
        None,
    );
    let rules = config["route"]["rules"].as_array().expect("rules");
    assert_eq!(rules.len(), 1, "no rules anywhere — only the private rule");
}

#[test]
fn build_run_config_dpi_fallback_targets_the_tunnel() {
    let endpoints = baked(&[json!({ "type": "vless", "server": "1.2.3.4", "server_port": 443 })]);
    let config = build_run_config(
        &endpoints,
        Some("mt-1"),
        MODE_OFF,
        7897,
        45678,
        Some((&routing_fixture("dpi"), Some(1080))),
        None,
    );
    assert_eq!(config["route"]["final"], DPI_TAG);
}

/// The raw local proxy: a second mixed inbound whose rule sits above
/// everything — the private-range direct rule included — and rides the
/// `mt-proxy` selector, so it always follows the selected endpoint.
#[test]
fn build_run_config_bakes_the_raw_proxy_above_all_rules() {
    let endpoints = baked(&[json!({ "type": "vless", "server": "1.2.3.4", "server_port": 443 })]);

    let config = build_run_config(
        &endpoints,
        Some("mt-1"),
        MODE_OFF,
        7897,
        45678,
        Some((&routing_fixture("proxy"), Some(1080))),
        Some(7890),
    );

    let inbounds = config["inbounds"].as_array().expect("inbounds");
    assert_eq!(inbounds.len(), 2);
    assert_eq!(inbounds[0]["tag"], "mixed-in", "the session port comes first");
    assert_eq!(inbounds[1]["type"], "mixed");
    assert_eq!(inbounds[1]["tag"], RAW_IN_TAG);
    assert_eq!(inbounds[1]["listen"], "127.0.0.1");
    assert_eq!(inbounds[1]["listen_port"], 7890);

    // the raw rule precedes even the private-range rule, and targets
    // the selector (not a concrete endpoint — live switches apply)
    let rules = config["route"]["rules"].as_array().expect("rules");
    assert_eq!(rules[0]["inbound"], json!([RAW_IN_TAG]));
    assert_eq!(rules[0]["outbound"], PROXY_TAG);
    assert_eq!(rules[1]["ip_is_private"], true);
    assert_eq!(rules[1]["outbound"], DIRECT_TAG);

    // without the setting neither the inbound nor its rule exist
    let config = build_run_config(
        &endpoints,
        Some("mt-1"),
        MODE_OFF,
        7897,
        45678,
        Some((&routing_fixture("proxy"), Some(1080))),
        None,
    );
    assert_eq!(config["inbounds"].as_array().expect("inbounds").len(), 1);
    assert!(
        !config["route"]["rules"]
            .as_array()
            .expect("rules")
            .iter()
            .any(|rule| rule.get("inbound").is_some())
    );

    // a DPI-only session has no selector to ride — no raw port either
    let config = build_run_config(
        &[],
        None,
        MODE_OFF,
        7897,
        45678,
        Some((&routing_fixture("dpi"), Some(1080))),
        Some(7890),
    );
    let inbounds = config["inbounds"].as_array().expect("inbounds");
    assert_eq!(inbounds.len(), 1);
    assert_eq!(inbounds[0]["tag"], "mixed-in");
    assert!(
        !config["route"]["rules"]
            .as_array()
            .expect("rules")
            .iter()
            .any(|rule| rule.get("inbound").is_some())
    );
}

/// A free configured port is kept; a busy one falls back to some other
/// (free) port instead of failing the connect.
#[test]
fn resolve_raw_port_falls_back_when_busy() {
    let free = latency::free_local_port().expect("free port");
    assert_eq!(resolve_raw_port(free), free);

    let holder = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("holder");
    let busy = holder.local_addr().expect("addr").port();
    let resolved = resolve_raw_port(busy);
    assert_ne!(resolved, busy, "a busy port must not be baked");
    assert!(resolved != 0);
    assert_ne!(resolved, free, "the fallback picks its own port");
}

/// DPI-only session (no endpoint): no proxy outbound, proxy actions
/// degrade to direct, dpi actions keep targeting the tunnel.
#[test]
fn build_run_config_dpi_only_degrades_proxy_to_direct() {
    let config = build_run_config(
        &[],
        None,
        MODE_OFF,
        7897,
        45678,
        Some((&routing_fixture("proxy"), Some(1080))),
        None,
    );

    // direct + dpi socks only — nothing references the proxy tag
    let outbounds = config["outbounds"].as_array().expect("outbounds");
    assert_eq!(outbounds.len(), 2);
    assert_eq!(outbounds[0]["type"], "direct");
    assert_eq!(outbounds[1]["tag"], DPI_TAG);
    assert_eq!(outbounds[1]["server_port"], 1080);

    // the dpi category keeps the tunnel; the proxy category degrades to direct
    let rules = config["route"]["rules"].as_array().expect("rules");
    let youtube = rules
        .iter()
        .find(|rule| rule.get("domain_suffix") == Some(&json!(["youtube.com", "youtu.be"])))
        .expect("the dpi rule survives");
    assert_eq!(youtube["outbound"], DPI_TAG);
    let speedtest = rules
        .iter()
        .find(|rule| {
            rule.get("domain_suffix")
                == Some(&json!(["speedtest.net", "media.example"]))
        })
        .expect("the proxy rule survives");
    assert_eq!(speedtest["outbound"], DIRECT_TAG);

    // a proxy fallback also degrades to direct
    assert_eq!(config["route"]["final"], DIRECT_TAG);

    // a dpi fallback still targets the tunnel
    let config = build_run_config(
        &[],
        None,
        MODE_OFF,
        7897,
        45678,
        Some((&routing_fixture("dpi"), Some(1080))),
        None,
    );
    assert_eq!(config["route"]["final"], DPI_TAG);
}
