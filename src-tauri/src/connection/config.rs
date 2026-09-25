//! The generated sing-box run config: every endpoint baked behind the
//! `mt-proxy` selector, the routing rules from the URL categories, the
//! optional raw local proxy port, and the temp-file plumbing.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

use crate::dpi;
use crate::latency;
use crate::routing;
use crate::sites;

use super::state::{DIRECT_TAG, DPI_TAG, MODE_TUN, PROXY_TAG};

/// Inbound tag of the raw local proxy port (Settings → General): everything
/// arriving on it rides the `mt-proxy` selector — the selected endpoint —
/// above every routing rule, private ranges included.
pub(super) const RAW_IN_TAG: &str = "mt-raw-in";

/// One endpoint baked into the run config behind the `mt-proxy` selector.
#[derive(Debug)]
pub(super) struct BakedEndpoint {
    /// endpoint row id — the live-switch map key
    pub(super) id: i64,
    /// synthetic outbound tag (`mt-1`, `mt-2`, … — unique, ASCII-only and
    /// never colliding with the reserved `mt-proxy` / `mt-direct` /
    /// `mt-dpi`)
    pub(super) tag: String,
    pub(super) outbound: Value,
}

/// Retags a profile's outbound rows into baked endpoints, in list order.
pub(super) fn bake_endpoints(rows: Vec<latency::OutboundRow>) -> Vec<BakedEndpoint> {
    rows.into_iter()
        .enumerate()
        .map(|(index, (id, outbound))| BakedEndpoint {
            id,
            tag: format!("mt-{}", index + 1),
            outbound,
        })
        .collect()
}

/// A DPI-only connect (no endpoint selected) is allowed exactly when a DPI
/// strategy is active and routing actually needs the tunnel — otherwise the
/// session would be an elaborate "direct" pipe. The caller additionally
/// requires the DPI master toggle to be on.
pub(super) fn dpi_only_allowed(
    strategy: Option<&dpi::DpiStrategy>,
    routing: &routing::RoutingConfig,
) -> bool {
    strategy.is_some() && routing::dpi_used(routing)
}

/// The real proxy config: every endpoint of the profile as an outbound
/// with a synthetic tag (all switchable without a restart), a `selector`
/// (tag `mt-proxy`, default = the selected endpoint) as the single outbound
/// the routing rules reference, a mixed inbound for manual/browser use,
/// the optional raw local proxy port (an extra mixed inbound whose rule
/// rides above everything, straight into the selector — see
/// `RAW_IN_TAG`), optional TUN, the optional DPI socks outbound, one
/// domain-suffix rule per URL category on top of "private ranges direct",
/// and the clash API for the readiness probe and live endpoint switches.
///
/// `default` is `None` in a DPI-only session: no proxy outbound exists,
/// and `proxy` actions (categories and fallback) degrade to direct. (No
/// pick also means no parseable endpoints, so there is nothing to bake
/// anyway.)
///
/// `routing` is `(config, dpi_port)`: `dpi_port` is `Some` only while the
/// ciadpi tunnel runs — dpi-routed categories (and the dpi fallback, which
/// the caller must reject earlier) are skipped without it.
pub(super) fn build_run_config(
    endpoints: &[BakedEndpoint],
    default: Option<&str>,
    mode: &str,
    mixed_port: u16,
    api_port: u16,
    routing: Option<(&routing::RoutingConfig, Option<u16>)>,
    raw_port: Option<u16>,
) -> Value {
    let mut outbounds: Vec<Value> = endpoints
        .iter()
        .map(|endpoint| {
            let mut outbound = endpoint.outbound.clone();
            latency::sanitize_outbound(&mut outbound);
            outbound["tag"] = Value::String(endpoint.tag.clone());
            outbound
        })
        .collect();
    // the selector is what makes endpoint switches restart-free: the Clash
    // API can repoint it at any baked outbound while the instance runs
    // (open connections drain on the previous outbound — interruptions
    // stay off)
    if let Some(default) = default {
        outbounds.push(json!({
            "type": "selector",
            "tag": PROXY_TAG,
            "outbounds": endpoints.iter().map(|endpoint| endpoint.tag.as_str()).collect::<Vec<_>>(),
            "default": default,
            "interrupt_exist_connections": false,
        }));
    }
    let has_proxy = default.is_some();
    // the raw port rides the selector: without one (DPI-only session) the
    // port is not opened at all
    let raw_port = raw_port.filter(|_| has_proxy);
    outbounds.push(json!({ "type": "direct", "tag": DIRECT_TAG }));
    if let Some((_, Some(dpi_port))) = routing {
        outbounds.push(json!({
            "type": "socks",
            "tag": DPI_TAG,
            "server": "127.0.0.1",
            "server_port": dpi_port,
            "version": "5",
        }));
    }

    // the raw port's rule sits above everything — the private-range direct
    // rule included: an app pointed at it wants *all* of its traffic on
    // the selected endpoint
    let mut rules = raw_port
        .map(|_| json!({ "inbound": [RAW_IN_TAG], "outbound": PROXY_TAG }))
        .into_iter()
        .collect::<Vec<_>>();
    rules.push(json!({ "ip_is_private": true, "outbound": DIRECT_TAG }));
    let final_tag = match routing {
        Some((config, dpi_port)) => {
            // the categories carry every domain rule: one sing-box rule per
            // (category, matcher kind), in category order (an overlapping
            // host is claimed by the first category that lists it)
            let category_rules: Vec<(&routing::RouteCategory, &str)> = config
                .categories
                .iter()
                .filter_map(|category| {
                    if category.rules.is_empty() {
                        return None;
                    }
                    let outbound = match category.action.as_str() {
                        routing::ACTION_DPI if dpi_port.is_some() => DPI_TAG,
                        routing::ACTION_DPI => return None,
                        routing::ACTION_PROXY if has_proxy => PROXY_TAG,
                        routing::ACTION_PROXY | routing::ACTION_DIRECT => DIRECT_TAG,
                        _ => return None,
                    };
                    Some((category, outbound))
                })
                .collect();
            // domain rules need the hostname: sniff it out of the TLS/HTTP
            // payload (TUN and bare-IP SOCKS connections carry no domain)
            if !category_rules.is_empty() {
                rules.push(json!({ "action": "sniff" }));
            }
            for (category, outbound) in category_rules {
                // bucket the typed rules per matcher kind — `url` rules
                // route by their hostname as a domain suffix
                let mut suffixes: Vec<String> = Vec::new();
                let mut domains: Vec<String> = Vec::new();
                let mut keywords: Vec<String> = Vec::new();
                let mut regexes: Vec<String> = Vec::new();
                for rule in &category.rules {
                    match rule.rule_type.as_str() {
                        sites::RULE_TYPE_DOMAIN_SUFFIX => suffixes.push(rule.value.clone()),
                        sites::RULE_TYPE_URL => {
                            suffixes.push(sites::host_of(&rule.value));
                        }
                        sites::RULE_TYPE_DOMAIN => domains.push(rule.value.clone()),
                        sites::RULE_TYPE_KEYWORD => keywords.push(rule.value.clone()),
                        sites::RULE_TYPE_REGEX => regexes.push(rule.value.clone()),
                        _ => {} // unknown types are filtered by the loader
                    }
                }
                for (field, values) in [
                    ("domain_suffix", &suffixes),
                    ("domain", &domains),
                    ("domain_keyword", &keywords),
                    ("domain_regex", &regexes),
                ] {
                    if !values.is_empty() {
                        rules.push(json!({ field: values, "outbound": outbound }));
                    }
                }
            }
            match config.fallback.as_str() {
                routing::ACTION_DIRECT => DIRECT_TAG,
                routing::ACTION_DPI if dpi_port.is_some() => DPI_TAG,
                _ if has_proxy => PROXY_TAG,
                _ => DIRECT_TAG,
            }
        }
        None if has_proxy => PROXY_TAG,
        None => DIRECT_TAG,
    };

    let mut inbounds = vec![json!({
        "type": "mixed",
        "tag": "mixed-in",
        "listen": "127.0.0.1",
        "listen_port": mixed_port,
    })];
    if let Some(raw_port) = raw_port {
        inbounds.push(json!({
            "type": "mixed",
            "tag": RAW_IN_TAG,
            "listen": "127.0.0.1",
            "listen_port": raw_port,
        }));
    }
    if mode == MODE_TUN {
        inbounds.push(json!({
            "type": "tun",
            "tag": "tun-in",
            "address": ["172.19.0.1/30", "fdfe:dcba:9876::1/126"],
            "auto_route": true,
            "strict_route": true,
            "stack": "system",
        }));
    }

    json!({
        "log": { "level": "warn" },
        "inbounds": inbounds,
        "outbounds": outbounds,
        "route": {
            "auto_detect_interface": true,
            "final": final_tag,
            "rules": rules,
        },
        "experimental": {
            "clash_api": { "external_controller": format!("127.0.0.1:{api_port}") }
        },
    })
}

pub(super) fn write_config(path: &PathBuf, config: &Value) -> Result<(), String> {
    let content =
        serde_json::to_string_pretty(config).map_err(|e| format!("failed to serialize config: {e}"))?;
    std::fs::write(path, content).map_err(|e| format!("failed to write {}: {e}", path.display()))
}

/// The port the raw local proxy listens on: the configured one when it is
/// free, a random free one otherwise (the snapshot carries the actual
/// port, so the UI never lies). Binding and dropping a listener is only a
/// probe — the real listener is sing-box's. A reconnect races the dying
/// previous instance for the port, so a busy port gets a short window to
/// let go before the fallback picks a stranger.
pub(super) fn resolve_raw_port(configured: u16) -> u16 {
    for attempt in 0..5 {
        if std::net::TcpListener::bind(("127.0.0.1", configured)).is_ok() {
            return configured;
        }
        if attempt < 4 {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    latency::free_local_port().unwrap_or(configured)
}
