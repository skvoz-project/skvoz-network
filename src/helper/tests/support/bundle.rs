//! Check the exact candidate bytes installed with its supplier manifest.
use serde_json::Value;
use std::{fs, path::Path, process::Command};
pub fn verify(paths: &[&str]) {
    let root = Path::new("/opt/skvoz-native");
    let manifest: Value =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["component"], "skvoz-network");
    assert_eq!(manifest["tuple"]["network"], 6);
    assert_eq!(manifest["tuple"]["core_abi"], 1);
    for path in paths {
        let record = manifest["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|file| file["path"] == *path)
            .unwrap();
        let digest = Command::new("/usr/bin/sha256sum")
            .arg(root.join(path))
            .output()
            .unwrap();
        assert!(digest.status.success());
        assert_eq!(
            String::from_utf8(digest.stdout)
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap(),
            record["sha256"].as_str().unwrap()
        );
    }
}
