//! Produce an immutable native bundle from already built supplier outputs.
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs, io,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn main() {
    if let Err(error) = run() {
        eprintln!("native artifact failed: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [root, built, core, output] = args.as_slice() else {
        return Err(
            "Usage: skvoz-network-artifact SOURCE BUILT_DIR CORE_LIBRARY OUTPUT_DIR".into(),
        );
    };
    let pin: serde_json::Value = serde_json::from_slice(&fs::read(
        root.join("dependencies/core/x86_64-unknown-linux-gnu.json"),
    )?)?;
    if pin["component"] != "skvoz-core"
        || pin["version"] != "5.0.0"
        || pin["abi"] != 1
        || pin["target"] != "x86_64-unknown-linux-gnu"
        || pin["sha256"] != sha256(core)?
        || pin["sdk_manifest_sha256"] != sha256(&root.join("vendor/skvoz-core/sdk.sha256"))?
    {
        return Err("native Core library or source SDK does not match the target pin".into());
    }
    if output.exists() {
        return Err("output directory already exists".into());
    }
    verify_outputs(built)?;
    let inventory = source_inventory(root)?;
    let inventory: String = inventory
        .into_iter()
        .map(|(path, digest)| format!("{digest}\t{path}\n"))
        .collect();
    fs::create_dir_all(output)?;
    fs::write(output.join("source-inputs.sha256"), inventory)?;
    let tree = sha256(&output.join("source-inputs.sha256"))?;
    let files = [
        (
            built.join("skvoz-network-runtime"),
            "bin/skvoz-network-runtime",
            0o755,
        ),
        (
            built.join("skvoz-network-helper"),
            "bin/skvoz-network-helper",
            0o755,
        ),
        (core.clone(), "lib/libskvoz_core.so", 0o644),
        (
            root.join("helper/systemd/skvoz-network-helper@.socket"),
            "resources/systemd/skvoz-network-helper@.socket",
            0o644,
        ),
        (
            root.join("helper/systemd/skvoz-network-helper@.service"),
            "resources/systemd/skvoz-network-helper@.service",
            0o644,
        ),
        (
            root.join("helper/systemd/skvoz-network-helper.conf"),
            "resources/systemd/skvoz-network-helper.conf",
            0o644,
        ),
        (
            root.join("helper/polkit/org.skvoz.network.policy"),
            "resources/polkit/org.skvoz.network.policy",
            0o644,
        ),
        (root.join("LICENSE"), "licenses/Network-MIT.txt", 0o644),
        (
            root.join("vendor/skvoz-core/LICENSE"),
            "licenses/Core-MIT.txt",
            0o644,
        ),
        (
            root.join("vendor/async-nats/LICENSE"),
            "licenses/async-nats-Apache-2.0.txt",
            0o644,
        ),
    ];
    let mut records = Vec::new();
    for (source, relative, mode) in files {
        if !fs::symlink_metadata(&source)?.is_file() {
            return Err("artifact input must be a regular file".into());
        }
        let path = output.join(relative);
        fs::create_dir_all(path.parent().ok_or("invalid artifact path")?)?;
        fs::copy(&source, &path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))?;
        records.push(json!({"path":relative,"sha256":sha256(&path)?,"mode":mode}));
    }
    fs::set_permissions(
        output.join("source-inputs.sha256"),
        fs::Permissions::from_mode(0o644),
    )?;
    records.push(json!({"path":"source-inputs.sha256","sha256":tree,"mode":0o644}));
    records.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    let manifest = json!({"schema":1,"component":"skvoz-network","version":"0.7.0",
        "target":"x86_64-unknown-linux-gnu","profile":"linux-gnu-bookworm",
        "tuple":{"api":1,"network":6,"core":"5.0.0","helper":"0.6.0","core_abi":1,"network_abi":1},
        "source":{"repository":"https://github.com/skvoz-project/skvoz-network","revision":source_revision(root)?,"tree_sha256":tree},
        "files":records,"dependencies":[{"component":"skvoz-core","version":"5.0.0","abi":1,"sha256":sha256(core)?}]});
    let manifest = serde_json::to_vec_pretty(&manifest)?;
    let bytes: u64 = records_size(output)? + manifest.len() as u64;
    if manifest.len() > 65536 || bytes > 256 * 1024 * 1024 {
        return Err("native bundle exceeds the supported size limit".into());
    }
    fs::write(output.join("manifest.json"), manifest)?;
    fs::set_permissions(
        output.join("manifest.json"),
        fs::Permissions::from_mode(0o644),
    )?;
    println!("{}", output.display());
    Ok(())
}
fn records_size(root: &Path) -> io::Result<u64> {
    let mut bytes = 0;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        bytes += if metadata.is_dir() {
            records_size(&entry.path())?
        } else {
            metadata.len()
        };
    }
    Ok(bytes)
}
fn source_inventory(root: &Path) -> Result<BTreeMap<String, String>> {
    let listed = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .output()?;
    if !listed.status.success() {
        return Err("source supplier requires an owning Git checkout".into());
    }
    let mut files = BTreeMap::new();
    for raw in listed
        .stdout
        .split(|b| *b == 0)
        .filter(|raw| !raw.is_empty())
    {
        let relative = std::str::from_utf8(raw)?;
        if ![
            "Cargo.toml",
            "Cargo.lock",
            "build.rs",
            "native-profile.json",
            "dependencies/",
            ".cargo/",
            ".github/",
            "rust-toolchain",
            "rust-toolchain.toml",
            "Dockerfile",
            "Dockerfile.native",
            "src/",
            "tests/",
            "contracts/",
            "native/",
            "helper/",
            "ffi/",
            "client/",
            "vendor/",
            "tools/",
            "scripts/",
            "qualification/",
        ]
        .iter()
        .any(|prefix| {
            if prefix.ends_with('/') {
                relative.starts_with(prefix)
            } else {
                relative == *prefix
            }
        }) {
            continue;
        }
        if relative.contains(['\n', '\t']) {
            return Err("invalid source path".into());
        }
        let path = root.join(relative);
        if !fs::symlink_metadata(&path)?.is_file() {
            return Err("source input must be a regular file".into());
        }
        files.insert(relative.to_owned(), sha256(&path)?);
    }
    if files.is_empty() {
        return Err("source input inventory is empty".into());
    }
    Ok(files)
}
fn sha256(path: &Path) -> Result<String> {
    let output = Command::new("sha256sum").arg("--").arg(path).output()?;
    if !output.status.success() {
        return Err(io::Error::other("sha256sum failed").into());
    }
    let text = String::from_utf8(output.stdout)?;
    let digest = text.split_whitespace().next().ok_or("missing SHA256")?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("invalid SHA256".into());
    }
    Ok(digest.into())
}

fn verify_outputs(built: &Path) -> Result<()> {
    for (name, expected) in [
        (
            "skvoz-network-runtime",
            "skvoz-network-runtime 0.7.0 network=6 api=1 core=5.0.0",
        ),
        (
            "skvoz-network-helper",
            "skvoz-network-helper 0.6.0 api=1 network=6",
        ),
    ] {
        let binary = built.join(name);
        let output = Command::new(&binary).arg("--version").output()?;
        if !output.status.success() || String::from_utf8(output.stdout)?.trim() != expected {
            return Err("native executable identity mismatch".into());
        }
        let elf = Command::new("readelf").arg("-d").arg(&binary).output()?;
        if !elf.status.success() {
            return Err("native executable is not ELF".into());
        }
        let versions = Command::new("readelf")
            .arg("--version-info")
            .arg(&binary)
            .output()?;
        if !versions.status.success() {
            return Err("cannot inspect native GLIBC requirements".into());
        }
        for token in String::from_utf8(versions.stdout)?.split_whitespace() {
            if let Some(version) = token.strip_prefix("GLIBC_") {
                let parts: Vec<_> = version
                    .split('.')
                    .map(str::parse::<u32>)
                    .collect::<std::result::Result<_, _>>()?;
                if parts.first() != Some(&2) || parts.get(1).is_none_or(|minor| *minor > 34) {
                    return Err("native executable requires GLIBC newer than 2.34".into());
                }
            }
        }
        let dynamic = String::from_utf8(elf.stdout)?;
        if name == "skvoz-network-runtime" {
            if !dynamic.contains("libskvoz_core.so") || !dynamic.contains("$ORIGIN/../lib") {
                return Err("runtime Core dependency or relative RPATH is missing".into());
            }
        } else if dynamic.contains("libskvoz_core") {
            return Err("helper must not link Core".into());
        }
    }
    Ok(())
}

fn source_revision(root: &Path) -> Result<Option<String>> {
    let head = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()?;
    let clean = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain"])
        .output()?;
    let revision = String::from_utf8(head.stdout)?.trim().to_owned();
    let qualified = head.status.success()
        && clean.status.success()
        && clean.stdout.is_empty()
        && revision.len() == 40
        && revision.bytes().all(|b| b.is_ascii_hexdigit());
    if let Ok(expected) = std::env::var("GITHUB_SHA")
        && (!qualified || expected != revision)
    {
        return Err("CI source must be the clean owning commit".into());
    }
    Ok(qualified.then_some(revision))
}
