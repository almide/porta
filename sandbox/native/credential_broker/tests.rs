use super::exchange::{outgoing_head, Framing};
use super::tls::civil_from_days;
use super::*;

fn credential(name: &str, value: &str, hosts: &[&str]) -> Credential {
    Credential {
        name: name.into(),
        value: value.into(),
        placeholder: format!("porta-cred-{name}-0123"),
        bindings: hosts.iter().map(|h| parse_binding(h).unwrap()).collect(),
    }
}

fn at(host: &str) -> Target<'_> {
    Target { host, port: 443 }
}

#[test]
fn bindings_parse_and_refuse_other_ports() {
    assert_eq!(parse_binding("API.example.com").unwrap(), Binding { host: "api.example.com".into(), port: 443, path: "".into() });
    assert_eq!(parse_binding("api.example.com:443/v1/").unwrap().path, "/v1");
    assert!(parse_binding("api.example.com:8443").is_err());
    assert!(parse_binding(":443").is_err());
}

#[test]
fn a_placeholder_is_put_on_only_where_it_is_bound() {
    let creds = vec![credential("KEY", "real-secret", &["api.example.com/v1"])];
    let head = "GET /v1/models?k=porta-cred-KEY-0123 HTTP/1.1\r\nx-api-key: porta-cred-KEY-0123\r\n\r\n";
    match rewrite_head(&creds, head, at("api.example.com"), "/v1/models?k=porta-cred-KEY-0123") {
        Rewrite::Send(out, used) => {
            assert!(!out.contains("porta-cred-KEY"));
            assert_eq!(out.matches("real-secret").count(), 2);
            assert_eq!(used, vec!["KEY".to_string()]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(rewrite_head(&creds, head, at("api.example.com"), "/admin"), Rewrite::Refuse("KEY".into()));
    assert_eq!(rewrite_head(&creds, head, at("evil.example.com"), "/v1/models"), Rewrite::Refuse("KEY".into()));
    assert!(!path_under("/v10", "/v1"));
}

#[test]
fn a_request_without_a_placeholder_goes_as_it_is() {
    let creds = vec![credential("KEY", "real-secret", &["api.example.com"])];
    let head = "GET / HTTP/1.1\r\nHost: api.example.com\r\n\r\n";
    assert_eq!(rewrite_head(&creds, head, at("api.example.com"), "/"), Rewrite::Send(head.into(), vec![]));
}

#[test]
fn the_outgoing_head_asks_for_close_and_knows_its_body() {
    let (head, framing) = outgoing_head("POST /v1 HTTP/1.1\r\nHost: a\r\nConnection: keep-alive\r\nExpect: 100-continue\r\nContent-Length: 12\r\n\r\n");
    assert!(head.ends_with("Connection: close\r\n\r\n") && !head.contains("keep-alive") && !head.contains("Expect"));
    assert_eq!(framing, Framing::Length(12));
    assert_eq!(outgoing_head("POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\nContent-Length: 3\r\n\r\n").1, Framing::Chunked);
}

#[test]
fn days_become_dates() {
    assert_eq!(civil_from_days(0), (1970, 1, 1));
    assert_eq!(civil_from_days(20_724), (2026, 9, 28));
    assert_eq!(civil_from_days(11_016), (2000, 2, 29));
}
