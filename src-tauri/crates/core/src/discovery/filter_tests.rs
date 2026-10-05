use super::filter::{dedup_endpoints, filter_lines, html_unescape, is_insecure_line};
use crate::parser::NewEndpoint;

fn endpoint(raw: &str, server: &str, server_port: u16) -> NewEndpoint {
    NewEndpoint {
        tag: raw.to_string(),
        protocol: "vless".to_string(),
        server: server.to_string(),
        server_port,
        raw: raw.to_string(),
        outbound_json: String::new(),
    }
}

#[test]
fn html_entities_decode() {
    assert_eq!(html_unescape("a&amp;b"), "a&b");
    assert_eq!(html_unescape("x&lt;y&gt;z"), "x<y>z");
    assert_eq!(html_unescape("&quot;a&#39;b&quot;"), "\"a'b\"");
    assert_eq!(html_unescape("&#59;"), ";");
    assert_eq!(html_unescape("&#x3B;"), ";");
    assert_eq!(html_unescape("&#X3b;"), ";");
    assert_eq!(html_unescape("&#47;path"), "/path");
    // no semicolon, unknown entity or plain ampersand stays literal
    assert_eq!(html_unescape("&amp"), "&amp");
    assert_eq!(html_unescape("&nope;"), "&nope;");
    assert_eq!(html_unescape("bare & and &unfinished"), "bare & and &unfinished");
    // multibyte text survives the byte scan
    assert_eq!(html_unescape("Ω&amp;λ"), "Ω&λ");
}

#[test]
fn insecure_parameter_forms_are_detected() {
    let insecure = [
        // mid-params, the common shape
        "vless://uuid@a.example:443?type=ws&allowInsecure=1&path=/x#tag",
        // underscore name, true value, end-of-string terminator
        "vless://uuid@a.example:443?allow_insecure=true",
        // bare name, uppercase value terminated by the fragment
        "vless://uuid@a.example:443?insecure=YES#frag",
        // yes mid-params
        "vless://uuid@a.example:443?insecure=yes&path=/x",
        // case-insensitive name and value
        "vless://uuid@a.example:443?ALLOWINSECURE=True",
        // percent-encoded separator and equals: ?security=tls&allowInsecure=1
        "vless://uuid@a.example:443?security=tls%26allowInsecure%3D1",
        // percent-encoded semicolon boundary
        "vless://uuid@a.example:443?security=tls%3BallowInsecure=1",
        // html-escaped boundary
        "vless://uuid@a.example:443?security=tls&amp;allowInsecure=1",
        // html-escaped terminator after the value
        "vless://uuid@a.example:443?allowInsecure=1&amp;sni=a.example",
        // html-escaped semicolon before the name
        "vless://uuid@a.example:443?security=tls&#59;allowInsecure=1",
        // the encoded-semicolon leftover boundary
        "vless://uuid@a.example:443?zz3%Ballowinsecure=1",
        "vless://uuid@a.example:443?zz%3bAllowInsecure=1",
        // semicolon separator, value terminated by whitespace
        "trojan://pwd@a.example:443?security=tls;insecure=1;sni=a.example",
        "vless://uuid@a.example:443?insecure=1 extra",
    ];
    for line in insecure {
        assert!(is_insecure_line(line), "should be filtered: {line}");
    }
}

#[test]
fn benign_lines_are_kept() {
    let kept = [
        "vless://uuid@a.example:443?type=ws&allowInsecure=0&path=/x#tag",
        "vless://uuid@a.example:443?allowInsecure=false",
        // substring without a parameter boundary
        "vless://uuid@a.example:443?notinsecure=1",
        // the value must terminate properly
        "vless://uuid@a.example:443?allowInsecure=trueX",
        "vless://uuid@a.example:443?allowInsecure=10",
        // no separator before the name at all
        "vless://uuid@a.example:443/path-allowinsecure=1",
        "vless://uuid@a.example:443?security=tls&sni=a.example#allowInsecure=1",
        "plain commentary line",
        "",
    ];
    for line in kept {
        assert!(!is_insecure_line(line), "should be kept: {line}");
    }
}

#[test]
fn filter_lines_drops_only_insecure_lines() {
    let content = "vless://uuid@a.example:443?allowInsecure=1\n\
                   vless://uuid@b.example:443?security=tls\n\
                   vless://uuid@c.example:443?insecure=true\n";
    let (kept, removed) = filter_lines(content);
    assert_eq!(removed, 2);
    assert_eq!(kept, "vless://uuid@b.example:443?security=tls");

    let (kept, removed) = filter_lines("");
    assert_eq!(removed, 0);
    assert_eq!(kept, "");
}

#[test]
fn dedup_keeps_first_by_raw_then_address() {
    let endpoints = vec![
        endpoint("vless://a", "one.example", 443),
        endpoint("vless://a", "one.example", 443),
        endpoint("vless://b", "ONE.example", 443),
        endpoint("vless://c", "one.example", 8443),
        endpoint("vless://d", "two.example", 443),
        endpoint("vless://e", "", 0),
        endpoint("vless://f", "", 0),
        endpoint("vless://g", "three.example", 0),
    ];
    let deduped = dedup_endpoints(endpoints);
    let raws: Vec<&str> = deduped.iter().map(|e| e.raw.as_str()).collect();
    assert_eq!(
        raws,
        vec!["vless://a", "vless://c", "vless://d", "vless://e", "vless://f", "vless://g"]
    );
}

#[test]
fn dedup_of_nothing_is_noop() {
    let deduped = dedup_endpoints(Vec::new());
    assert!(deduped.is_empty());
}
