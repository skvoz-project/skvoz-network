//! Real ready-byte packaging, selected explicitly with an existing supplier archive.
use std::{fs, path::PathBuf, process::Command};

fn output(command: &mut Command) -> Vec<u8> {
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}
fn hash(path: &std::path::Path) -> String {
    String::from_utf8(output(Command::new("sha256sum").arg(path)))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .into()
}
#[test]
#[ignore = "requires an exact qualified native archive; no engine compilation"]
fn bundle_reuses_ready_bytes_without_shipping_a_consumer_installer() {
    let archive = PathBuf::from(std::env::var_os("SKVOZ_TEST_READY_NATIVE_ARCHIVE").unwrap());
    assert_eq!(
        hash(&archive),
        std::env::var("SKVOZ_TEST_READY_NATIVE_SHA256").unwrap()
    );
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let directory = String::from_utf8(output(Command::new("mktemp").arg("-d"))).unwrap();
    let directory = PathBuf::from(directory.trim());
    let ready = directory.join("ready");
    let extracted = directory.join("extracted");
    fs::create_dir(&ready).unwrap();
    fs::create_dir(&extracted).unwrap();
    output(
        Command::new("tar")
            .args(["-xzf"])
            .arg(&archive)
            .arg("-C")
            .arg(&ready)
            .args([
                "--",
                "bin/skvoz-network-runtime",
                "bin/skvoz-network-helper",
                "lib/libskvoz_core.so",
            ]),
    );
    let bundle = directory.join("network.tar.gz");
    output(
        Command::new("sh")
            .arg(root.join("src/tools/bundle-native.sh"))
            .arg(env!("CARGO_BIN_EXE_skvoz-network-artifact"))
            .arg(ready.join("bin"))
            .arg(ready.join("lib/libskvoz_core.so"))
            .arg(&bundle),
    );
    output(
        Command::new("tar")
            .arg("-xzf")
            .arg(&bundle)
            .arg("-C")
            .arg(&extracted),
    );
    for name in [
        "bin/skvoz-network-runtime",
        "bin/skvoz-network-helper",
        "lib/libskvoz_core.so",
    ] {
        assert_eq!(hash(&ready.join(name)), hash(&extracted.join(name)));
    }
    assert!(!extracted.join("bin/skvoz-network-artifact").exists());
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(extracted.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["files"].as_array().unwrap().len(), 11);
    for file in manifest["files"].as_array().unwrap() {
        assert_eq!(
            file["sha256"].as_str().unwrap(),
            hash(&extracted.join(file["path"].as_str().unwrap()))
        );
    }
    fs::remove_dir_all(directory).unwrap();
}
