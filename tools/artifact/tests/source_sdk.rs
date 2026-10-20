//! Exercise the real portable exporter, including root without CAP_CHOWN in CI.
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
fn portable_export_preserves_source_and_normalizes_archive_ownership() {
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
    .filter(|s| !s.is_empty())
    .map(str::to_owned)
    .collect();
    let revision = if git(&["status", "--porcelain"]).is_empty() {
        Value::String(git(&["rev-parse", "HEAD"]).trim().into())
    } else {
        Value::Null
    };
    output(
        Command::new("sh")
            .arg(repo.join("scripts/export-source-sdk.sh"))
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
        hash(&repo.join("vendor/skvoz-core/sdk.sha256"))
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
        assert_eq!(
            fs::read(extracted.join(name)).unwrap(),
            fs::read(repo.join(name)).unwrap()
        );
        let mode = fs::metadata(repo.join(name)).unwrap().permissions().mode();
        assert_eq!(
            fs::metadata(extracted.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            if mode & 0o111 == 0 { 0o644 } else { 0o755 }
        );
    }
    assert_eq!(actual, inputs);
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
