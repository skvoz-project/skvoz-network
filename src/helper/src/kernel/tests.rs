use super::*;
#[test]
fn nft_ownership_requires_exact_table_header_comment() {
    let token = "00000000000000000000000000000000";
    let comment = format!("comment \"skvoz:{token}\"");
    assert!(owned_table_header(
        format!("table inet skvoz_network {{\n\t{comment}\n\tchain guard {{\n}}\n}}\n").as_bytes(),
        token
    ));
    for text in [
        format!("table ip skvoz_network {{\n{comment}\n}}"),
        format!("table inet foreign {{\n{comment}\n}}"),
        "table inet skvoz_network {\ncomment \"skvoz:foreign\"\n}".into(),
        format!("table inet skvoz_network {{\nchain guard {{\n{comment}\n}}\n}}"),
        format!("table inet skvoz_network {{\n{comment} extra\n}}"),
        "table inet skvoz_network {\n}".into(),
    ] {
        assert!(!owned_table_header(text.as_bytes(), token));
    }
    assert!(!owned_table_header(b"\xff", token));
}
#[test]
fn empty_conntrack_is_not_inferred_from_a_partial_error_diagnostic() {
    assert!(empty_conntrack_match(
        b"conntrack v1.4.7 (conntrack-tools): 0 flow entries have been deleted.\n"
    ));
    for body in [b"0 flow entries have been deleted".as_slice(),
        b"conntrack v1.4.7 (conntrack-tools): 0 flow entries have been deleted.\nPermission denied\n",
        b"Permission denied: 0 flow entries have been deleted"] {
        assert!(!empty_conntrack_match(body));
    }
}
#[test]
fn numerical_json_and_route_prefixes_are_strict() {
    assert_eq!(number(Some(&serde_json::json!("186"))), Some(186));
    assert_eq!(number(Some(&serde_json::json!(186))), Some(186));
    assert_eq!(number(Some(&serde_json::json!("boot"))), None);
    assert_eq!(
        parse_route_prefix(&serde_json::json!({"dst":"default"}), 6).unwrap(),
        "::/0".parse().unwrap()
    );
    assert_eq!(
        parse_route_prefix(&serde_json::json!({"dst":"10.203.0.2"}), 4).unwrap(),
        "10.203.0.2/32".parse().unwrap()
    );
    assert!(parse_route_prefix(&serde_json::json!({"dst":"somewhere"}), 4).is_err());
}
#[test]
fn kernel_unreachable_pool_routes_accept_numeric_types_and_ipv6_loopback() {
    for route in [
        serde_json::json!({"type":"7"}),
        serde_json::json!({"type":7,"dev":"lo"}),
        serde_json::json!({"type":"unreachable","dev":"lo"}),
    ] {
        assert!(unreachable_pool_route(&route));
    }
    for route in [
        serde_json::json!({"type":"unicast","dev":"lo"}),
        serde_json::json!({"type":"7","dev":"eth0"}),
        serde_json::json!({"type":"7","gateway":"192.0.2.1"}),
        serde_json::json!({"dev":"lo"}),
    ] {
        assert!(!unreachable_pool_route(&route));
    }
}
