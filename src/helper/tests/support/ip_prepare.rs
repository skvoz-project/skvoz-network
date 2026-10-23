//! Generate one short-lived private fixture input volume before actors start.
use super::{E, ROOT, U, V, private};
use serde_json::{Value, json};
use skvoz_network_contracts::{
    SessionId,
    config::{Limits, Role},
};
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};
pub(super) fn inputs(root: &Path) {
    fs::create_dir_all(root).unwrap();
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    let tls = root.join("tls");
    fs::create_dir(&tls).unwrap();
    fs::set_permissions(&tls, fs::Permissions::from_mode(0o700)).unwrap();
    let openssl = |args: &[&str]| {
        let output = Command::new("/usr/bin/openssl")
            .current_dir(&tls)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "OpenSSL fixture preparation: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    openssl(&[
        "req",
        "-x509",
        "-newkey",
        "rsa:2048",
        "-nodes",
        "-days",
        "1",
        "-subj",
        "/CN=Network fixture CA",
        "-keyout",
        "ca.key",
        "-out",
        "ca.pem",
    ]);
    openssl(&[
        "req",
        "-newkey",
        "rsa:2048",
        "-nodes",
        "-subj",
        "/CN=localhost",
        "-keyout",
        "leaf.key",
        "-out",
        "leaf.csr",
    ]);
    private(&tls.join("leaf.ext"),b"basicConstraints=critical,CA:FALSE\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:localhost\n");
    openssl(&[
        "x509",
        "-req",
        "-in",
        "leaf.csr",
        "-CA",
        "ca.pem",
        "-CAkey",
        "ca.key",
        "-CAcreateserial",
        "-out",
        "leaf.pem",
        "-days",
        "1",
        "-extfile",
        "leaf.ext",
    ]);
    for entry in fs::read_dir(&tls).unwrap() {
        fs::set_permissions(entry.unwrap().path(), fs::Permissions::from_mode(0o600)).unwrap();
    }
    let ca = fs::read_to_string(tls.join("ca.pem")).unwrap();
    let namespace = format!("skvoz.topology.{}", SessionId::random().unwrap().as_str());
    let password = SessionId::random().unwrap().as_str().to_owned();
    let users: Vec<_> = (0..=1)
        .map(|peer| {
            let (publish, subscribe) = if peer == 0 {
                (
                    vec![
                        format!("{namespace}.join.*.0"),
                        format!("{namespace}.lane.*.*.0.*.0.*"),
                    ],
                    vec![
                        format!("{namespace}.join.0.*"),
                        format!("{namespace}.lane.0.*.*.*.*.*"),
                    ],
                )
            } else {
                (
                    vec![
                        format!("{namespace}.join.0.1"),
                        format!("{namespace}.lane.0.*.1.*.1.*"),
                    ],
                    vec![
                        format!("{namespace}.join.1.*"),
                        format!("{namespace}.lane.1.*.*.*.*.*"),
                    ],
                )
            };
            json!({"user":format!("p{peer}"),"password":password,
            "permissions":{"publish":publish,"subscribe":subscribe}})
        })
        .collect();
    let nats = json!({
        "host":format!("{U}2"),"port":4222,"max_payload":65588,"max_pending":4194304,
        "tls":{"cert_file":format!("{ROOT}/leaf.pem"),"key_file":format!("{ROOT}/leaf.key"),"handshake_first":false},
        "authorization":{"users":users}
    });
    for role in ["server", "client", "endpoint"] {
        let directory = root.join(role);
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let value = if role == "endpoint" {
            json!({"leaf_pem":fs::read_to_string(tls.join("leaf.pem")).unwrap(),"leaf_key":fs::read_to_string(tls.join("leaf.key")).unwrap(),"nats":nats})
        } else {
            let server = role == "server";
            let mut config: Value = serde_json::from_str(include_str!(
                "../../../network/tests/fixtures/client-startup.json"
            ))
            .unwrap();
            config["core"]["url"] = json!(format!("tls://{U}2:4222"));
            config["core"]["tls_server_name"] = json!("localhost");
            config["core"]["trust"] = json!("managed_ca");
            config["core"]["ca_file"] = json!(format!("{ROOT}/profile/ca.pem"));
            config["core"]["username"] = json!(if server { "p0" } else { "p1" });
            config["core"]["password"] = json!(password);
            config["core"]["namespace"] = json!(namespace);
            config["core"]["peer_id"] = json!(if server { "0" } else { "1" });
            config["core"]["allowed_peers"] = json!(if server { vec!["1"] } else { vec!["0"] });
            config["core"]["initiate"] = json!(if server {
                Vec::<&str>::new()
            } else {
                vec!["0"]
            });
            config["network"]["families"] = json!([4, 6]);
            if server {
                config["role"] = json!("server");
                config["access"] = json!({"revision":1,"devices":[1],"revoke":[]});
                config["network"]["max_mtu"] = json!(1400);
                config["network"]["limits"] =
                    serde_json::to_value(Limits::canonical(Role::Server)).unwrap();
                config["server"] = json!({
                    "ipv4":{"pool":"10.203.0.0/24","egress":"nat44","interface":"eth1"},
                    "ipv6":{"pool":"2001:db8:203::/64","egress":"routed","interface":"eth1"},
                    "dns_servers":[format!("{E}30")],
                    "allow":[
                        {"cidr":"198.19.0.0/24","protocols":"any","ports":null},
                        {"cidr":"2001:db8:900::/64","protocols":"any","ports":null}
                    ],
                    "deny":[],"service_prefixes":[],"lease_store":"/var/lib/skvoz-network/leases.json",
                    "server_addresses":[format!("{U}20"),format!("{E}20"),format!("{V}20")],
                    "management_endpoints":[{"address":format!("{U}20"),"protocol":6,"port":4222}]
                });
            }
            json!({"runtime":config,"ca_pem":ca})
        };
        private(
            &directory.join("master.json"),
            &serde_json::to_vec(&value).unwrap(),
        );
    }
    // CA private material is not needed by any live actor.
    fs::remove_dir_all(tls).unwrap();
    println!("Private topology inputs prepared");
}
