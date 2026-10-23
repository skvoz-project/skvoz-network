//! Exercise the real portable production exporter under root without CAP_CHOWN.
use serde_json::Value;
use std::{collections::BTreeSet, fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

fn output(command: &mut Command) -> String {
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}
fn hash(path: &Path) -> String {
    output(Command::new("sha256sum").arg(path))
        .split_whitespace()
        .next()
        .unwrap()
        .into()
}

#[test]
fn portable_export_has_exact_production_closure_and_normalized_ownership() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let scratch = std::path::PathBuf::from(output(Command::new("mktemp").arg("-d")).trim());
    let git = |args: &[&str]| output(Command::new("git").arg("-C").arg(&repo).args(args));
    let inputs: BTreeSet<_> = git(&[
        "ls-files",
        "--cached",
        "--others",
        "--exclude-standard",
        "-z",
    ])
    .split('\0')
    .filter_map(|path| path.strip_prefix("src/"))
    .filter_map(|path| {
        if path.starts_with("network/src/") && !path.starts_with("network/src/bin/")
            || path == "network/Cargo.toml"
            || path == "network/build.rs"
        {
            Some(path.strip_prefix("network/").unwrap().to_owned())
        } else if path == "Cargo.lock"
            || ["contracts", "native"].iter().any(|package| {
                path == format!("{package}/Cargo.toml")
                    || path.starts_with(&format!("{package}/src/"))
            })
            || path.starts_with("vendor/skvoz-core/")
            || path.starts_with("vendor/async-nats/src/")
            || ["Cargo.toml", "README.md", "LICENSE", "PATCHES.md"]
                .iter()
                .any(|name| path == format!("vendor/async-nats/{name}"))
            || ["aarch64-linux-android", "x86_64-linux-android"]
                .iter()
                .any(|target| {
                    path == format!("dependencies/core/{target}.json")
                        || path == format!("dependencies/core/descriptors/{target}.json")
                })
        {
            Some(path.to_owned())
        } else {
            None
        }
    })
    .chain(["LICENSE".to_owned(), "README.md".to_owned()])
    .collect();
    let revision = if git(&["status", "--porcelain"]).is_empty() {
        Value::String(git(&["rev-parse", "HEAD"]).trim().into())
    } else {
        Value::Null
    };
    output(
        Command::new(env!("CARGO_BIN_EXE_export-sdk"))
            .arg(&repo)
            .arg("portable-supplier")
            .arg(scratch.join("output")),
    );
    let archive = scratch.join("output/skvoz-network-sdk-0.7.0-portable-supplier.tar.gz");
    let listing = output(
        Command::new("tar")
            .args(["--numeric-owner", "-tvzf"])
            .arg(&archive),
    );
    assert!(
        listing
            .lines()
            .all(|line| line.split_whitespace().nth(1) == Some("0/0"))
    );
    let extracted = scratch.join("extracted");
    fs::create_dir(&extracted).unwrap();
    output(
        Command::new("tar")
            .args(["--no-same-owner", "-xzf"])
            .arg(&archive)
            .arg("-C")
            .arg(&extracted),
    );
    let manifest: Value =
        serde_json::from_slice(&fs::read(extracted.join("sdk.json")).unwrap()).unwrap();
    assert_eq!(manifest["component"], "skvoz-network-sdk");
    assert_eq!(manifest["version"], "0.7.0");
    assert_eq!(manifest["profile"], "portable-supplier");
    assert_eq!(
        manifest["source"]["repository"],
        "https://github.com/skvoz-project/skvoz-network"
    );
    assert_eq!(manifest["source"]["revision"], revision);
    assert_eq!(
        manifest["source"]["tree_sha256"],
        hash(&extracted.join("source-inputs.sha256"))
    );
    assert_eq!(
        manifest["dependencies"][0]["manifest_sha256"],
        hash(&repo.join("src/vendor/skvoz-core/sdk.sha256"))
    );
    assert_eq!(
        fs::read(extracted.join("sdk.json")).unwrap(),
        fs::read(scratch.join("output/sdk.json")).unwrap()
    );
    let inventory = fs::read_to_string(extracted.join("source-inputs.sha256")).unwrap();
    let mut actual = BTreeSet::new();
    for line in inventory.lines() {
        let (digest, name) = line.split_once('\t').unwrap();
        assert!(actual.insert(name.to_owned()));
        assert_eq!(digest, hash(&extracted.join(name)));
        if !["Cargo.toml", "Cargo.lock", "README.md", "build.rs"].contains(&name) {
            assert_eq!(
                fs::read(extracted.join(name)).unwrap(),
                fs::read(if name == "LICENSE" {
                    repo.join(name)
                } else if name.starts_with("src/") {
                    repo.join("src/network").join(name)
                } else {
                    repo.join("src").join(name)
                })
                .unwrap()
            );
        }
        assert_eq!(
            fs::metadata(extracted.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
        assert!(!name.split('/').any(|part| {
            [
                "tests",
                "helper",
                "qualification",
                "ffi",
                "tools",
                "scripts",
                ".github",
            ]
            .contains(&part)
        }));
    }
    let mut inputs = inputs;
    inputs.insert("README.md".into());
    assert_eq!(actual, inputs);
    let cargo = fs::read_to_string(extracted.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("members = [\".\", \"contracts\", \"native\"]"));
    assert!(!cargo.contains("[[bin]]"));
    assert!(!cargo.contains("[dev-dependencies]"));
    assert!(!cargo.contains("qualification"));
    let lock_before = fs::read(extracted.join("Cargo.lock")).unwrap();
    output(
        Command::new("cargo")
            .args(["metadata", "--locked", "--offline", "--format-version", "1"])
            .arg("--manifest-path")
            .arg(extracted.join("Cargo.toml")),
    );
    assert_eq!(fs::read(extracted.join("Cargo.lock")).unwrap(), lock_before);
    let names: BTreeSet<_> = output(Command::new("tar").arg("-tzf").arg(&archive))
        .lines()
        .map(str::to_owned)
        .collect();
    actual.extend(["sdk.json".into(), "source-inputs.sha256".into()]);
    assert_eq!(names, actual);
    assert_eq!(
        fs::read_to_string(scratch.join("output/archive.sha256"))
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap(),
        hash(&archive)
    );
    fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn desktop_export_does_not_distribute_foreign_test_targets() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let scratch = std::path::PathBuf::from(output(Command::new("mktemp").arg("-d")).trim());
    output(
        Command::new(env!("CARGO_BIN_EXE_export-sdk"))
            .arg(&repo)
            .arg("desktop-host")
            .arg(scratch.join("output")),
    );
    let archive = scratch.join("output/skvoz-network-sdk-0.7.0-desktop-host.tar.gz");
    let names = output(Command::new("tar").arg("-tzf").arg(&archive));
    assert!(!names.lines().any(|path| path.split('/').any(|part| {
        [
            "tests",
            "bin",
            "helper",
            "qualification",
            "ffi",
            "tools",
            "scripts",
            ".github",
        ]
        .contains(&part)
    })));
    for path in names.lines() {
        assert!(
            [
                "LICENSE",
                "Cargo.lock",
                "README.md",
                "sdk.json",
                "source-inputs.sha256",
                "vendor/skvoz-core/LICENSE"
            ]
            .contains(&path)
                || [
                    "contracts",
                    "client",
                    "native",
                    "vendor/skvoz-core/contracts"
                ]
                .iter()
                .any(|package| path == format!("{package}/Cargo.toml")
                    || path.starts_with(&format!("{package}/src/")))
        );
    }
    let manifest = output(
        Command::new("tar")
            .arg("-xOzf")
            .arg(&archive)
            .arg("client/Cargo.toml"),
    );
    assert!(!manifest.contains("[[test]]"));
    assert!(!manifest.contains(".workspace"));
    fs::remove_dir_all(scratch).unwrap();
}
