//! Fetch exact Core releases; validate before installing dependency pins or artifacts.
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const REPO: &str = "https://github.com/skvoz-project/skvoz-core";
const TARGETS: [&str; 3] = [
    "x86_64-unknown-linux-gnu",
    "aarch64-linux-android",
    "x86_64-linux-android",
];
fn main() {
    if let Err(error) = run() {
        eprintln!("Core release failed: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let mut args: Vec<_> = std::env::args().skip(1).collect();
    let public = args.first().is_some_and(|arg| arg == "--public");
    if public {
        args.remove(0);
    }
    let root = std::env::current_dir()?;
    let scratch = Scratch::new()?;
    match args.as_slice() {
        [mode, tag] if mode == "refresh" => refresh(&root, &scratch.0, tag, public),
        [mode, target, output] if mode == "fetch" && TARGETS.contains(&target.as_str()) => fetch(&root, &scratch.0, target, output, public),
        _ => Err("Usage: core-release [--public] refresh EXACT_TAG | fetch TARGET OUTPUT_DIR (from repository root)".into()),
    }
}
fn refresh(root: &Path, scratch: &Path, tag: &str, public: bool) -> Result<()> {
    revision(tag)?;
    verify_remote(tag, public)?;
    download(tag, "SHA256SUMS", scratch, public)?;
    let pins = scratch.join("pins");
    fs::create_dir_all(pins.join("descriptors"))?;
    let mut sdk_digest = None;
    for target in TARGETS {
        let archive = archive_name(target);
        download(tag, &archive, scratch, public)?;
        let digest = checksum(&scratch.join("SHA256SUMS"), &archive)?;
        let artifact = scratch.join(target);
        unpack(&scratch.join(&archive), &digest, &artifact)?;
        let manifest = verify(&artifact, target, revision(tag)?)?;
        let sdk_hash = manifest["sdk_manifest_sha256"]
            .as_str()
            .ok_or("missing SDK digest")?;
        if sdk_digest.as_deref().is_some_and(|old| old != sdk_hash) {
            return Err("Core targets disagree on source SDK".into());
        }
        sdk_digest = Some(sdk_hash.to_owned());
        let mut pin: Value = read_json(&root.join(format!("dependencies/core/{target}.json")))?;
        for field in [
            "schema",
            "component",
            "version",
            "abi",
            "kind",
            "target",
            "artifact_id",
            "sha256",
            "sdk_manifest_sha256",
            "source_inputs_sha256",
        ] {
            pin[field] = if field == "schema" {
                json!(1)
            } else {
                manifest[field].clone()
            };
        }
        pin["artifact_manifest_sha256"] = json!(hash(&artifact.join("artifact.json"))?);
        pin["source_revision"] = json!(revision(tag)?);
        pin["artifact"] =
            json!({"url":format!("{REPO}/releases/download/{tag}/{archive}"), "sha256":digest});
        pin["release_tag"] = json!(tag);
        if target.ends_with("-android") {
            verify_android(&pin, &manifest)?;
            fs::copy(
                artifact.join("artifact.json"),
                pins.join(format!("descriptors/{target}.json")),
            )?;
        }
        fs::write(
            pins.join(format!("{target}.json")),
            format!("{}\n", serde_json::to_string_pretty(&pin)?),
        )?;
    }
    // Stage next to destination so rename stays on the owning filesystem.
    let prepared_pins = root.join("dependencies/.core-release-next");
    let prepared_sdk = root.join("vendor/.core-release-next");
    if prepared_pins.exists() || prepared_sdk.exists() {
        return Err("unfinished Core refresh staging exists".into());
    }
    copy_tree(&pins, &prepared_pins)?;
    if let Err(error) = copy_tree(&scratch.join(TARGETS[0]).join("sdk"), &prepared_sdk) {
        fs::remove_dir_all(&prepared_pins)?;
        return Err(error);
    }
    replace_pair(
        &root.join("dependencies/core"),
        &prepared_pins,
        &root.join("vendor/skvoz-core"),
        &prepared_sdk,
    )?;
    println!("Updated all Core pins and vendored SDK from {tag}");
    Ok(())
}
fn fetch(root: &Path, scratch: &Path, target: &str, output: &str, public: bool) -> Result<()> {
    let pin: Value = read_json(&root.join(format!("dependencies/core/{target}.json")))?;
    let tag = pin["release_tag"]
        .as_str()
        .ok_or("Core pin has no published release; run refresh after Core publication")?;
    let sha = revision(tag)?;
    if pin["source_revision"] != sha {
        return Err("Core pin source does not match release tag".into());
    }
    let archive = archive_name(target);
    if pin["artifact"]["url"] != format!("{REPO}/releases/download/{tag}/{archive}") {
        return Err("Core release URL mismatch".into());
    }
    verify_remote(tag, public)?;
    download(tag, "SHA256SUMS", scratch, public)?;
    download(tag, &archive, scratch, public)?;
    let digest = checksum(&scratch.join("SHA256SUMS"), &archive)?;
    if pin["artifact"]["sha256"] != digest {
        return Err("Core release checksum changed".into());
    }
    let artifact = scratch.join("artifact");
    unpack(&scratch.join(archive), &digest, &artifact)?;
    let manifest = verify(&artifact, target, sha)?;
    for field in [
        "component",
        "version",
        "abi",
        "kind",
        "target",
        "artifact_id",
        "sha256",
        "sdk_manifest_sha256",
        "source_inputs_sha256",
    ] {
        if pin[field] != manifest[field] {
            return Err(format!("Core pin mismatch: {field}").into());
        }
    }
    if target.ends_with("-android") {
        verify_android(&pin, &manifest)?;
    }
    if pin["artifact_manifest_sha256"] != hash(&artifact.join("artifact.json"))?
        || pin["sdk_manifest_sha256"] != hash(&root.join("vendor/skvoz-core/sdk.sha256"))?
    {
        return Err("Core descriptor or vendored SDK mismatch".into());
    }
    inventory(&root.join("vendor/skvoz-core"), "sdk.sha256", true)?;
    copy_tree(&artifact, Path::new(output))?;
    println!("Verified {tag} {target}");
    Ok(())
}
fn verify_android(pin: &Value, manifest: &Value) -> Result<()> {
    for key in [
        "android_abi",
        "minimum_api",
        "page_alignment",
        "ndk_revision",
        "elf_machine",
    ] {
        if pin["android"][key].is_null() || pin["android"][key] != manifest["android"][key] {
            return Err(format!("Core Android profile changed: {key}").into());
        }
    }
    Ok(())
}
fn revision(tag: &str) -> Result<&str> {
    let sha = tag
        .strip_prefix("core-v5.0.0-")
        .ok_or("expected exact core-v5.0.0-FULL40SHA tag")?;
    if sha.len() != 40
        || !sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("expected lowercase full40 source SHA".into());
    }
    Ok(sha)
}
fn verify_remote(tag: &str, public: bool) -> Result<()> {
    for (endpoint, pointer, error) in [
        (
            format!("git/ref/tags/{tag}"),
            "/object/sha",
            "Core tag must resolve directly to its owning commit",
        ),
        (
            format!("commits/{}", revision(tag)?),
            "/sha",
            "Core source commit is unavailable",
        ),
    ] {
        let actual = if public {
            let response = output(&mut public_request(&format!(
                "https://api.github.com/repos/skvoz-project/skvoz-core/{endpoint}"
            )))?;
            let value: Value = serde_json::from_str(&response)?;
            value
                .pointer(pointer)
                .and_then(Value::as_str)
                .ok_or(error)?
                .to_owned()
        } else {
            output(
                Command::new("timeout")
                    .args(["--kill-after=5s", "120s", "gh", "api"])
                    .arg(format!("repos/skvoz-project/skvoz-core/{endpoint}"))
                    .args(["--jq", &pointer.replace('/', ".")]),
            )?
        };
        if actual.trim() != revision(tag)? {
            return Err(error.into());
        }
    }
    Ok(())
}
fn public_request(url: &str) -> Command {
    let mut command = Command::new("timeout");
    command.args([
        "--kill-after=5s",
        "120s",
        "curl",
        "--disable",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--fail",
        "--location",
        "--silent",
        "--show-error",
        "--connect-timeout",
        "10",
        "--max-time",
        "115",
        url,
    ]);
    command
}
fn archive_name(target: &str) -> String {
    format!("skvoz-core-5.0.0-{target}.tar.gz")
}
fn download(tag: &str, asset: &str, dir: &Path, public: bool) -> Result<()> {
    if public {
        return status(
            public_request(&format!("{REPO}/releases/download/{tag}/{asset}"))
                .arg("--output")
                .arg(dir.join(asset)),
        );
    }
    status(
        Command::new("timeout")
            .args(["--kill-after=5s", "120s", "gh"])
            .args([
                "release",
                "download",
                tag,
                "--repo",
                "skvoz-project/skvoz-core",
                "--pattern",
                asset,
                "--dir",
            ])
            .arg(dir),
    )
}
fn checksum(path: &Path, name: &str) -> Result<String> {
    let contents = fs::read_to_string(path)?;
    let mut matches = contents
        .lines()
        .filter_map(|line| line.split_once("  "))
        .filter(|(_, file)| *file == name);
    let (digest, _) = matches.next().ok_or("missing release checksum")?;
    if matches.next().is_some() || !valid_hash(digest) {
        return Err("invalid or duplicate release checksum".into());
    }
    Ok(digest.to_owned())
}
fn unpack(archive: &Path, expected: &str, dir: &Path) -> Result<()> {
    if !valid_hash(expected) || hash(archive)? != expected {
        return Err("Core archive SHA256 mismatch".into());
    }
    let mut child = Command::new("gzip")
        .args(["-cd", "--"])
        .arg(archive)
        .stdout(Stdio::piped())
        .spawn()?;
    let bytes = io::copy(
        &mut child
            .stdout
            .take()
            .ok_or("gzip stdout missing")?
            .take(256 * 1024 * 1024 + 1),
        &mut io::sink(),
    )?;
    if bytes > 256 * 1024 * 1024 {
        child.kill()?;
        child.wait()?;
        return Err("Core archive exceeds size limit".into());
    }
    if !child.wait()?.success() {
        return Err("invalid gzip archive".into());
    }
    let listing = output(Command::new("tar").arg("-tzf").arg(archive))?;
    let types = output(Command::new("tar").arg("-tvzf").arg(archive))?;
    if types.lines().any(|line| !line.starts_with(['-', 'd'])) {
        return Err("Core archive contains links or special files".into());
    }
    let mut seen = BTreeSet::new();
    for name in listing.lines() {
        let name = name
            .strip_prefix("./")
            .unwrap_or(name)
            .trim_end_matches('/');
        if name.is_empty() {
            continue;
        }
        if !safe_path(name) || !seen.insert(name.to_owned()) || seen.len() > 4096 {
            return Err("invalid or duplicate Core archive path".into());
        }
    }
    fs::create_dir(dir)?;
    status(
        Command::new("tar")
            .args(["--no-same-owner", "--no-same-permissions", "-xzf"])
            .arg(archive)
            .arg("-C")
            .arg(dir),
    )
}
fn verify(root: &Path, target: &str, sha: &str) -> Result<Value> {
    let value = read_json(&root.join("artifact.json"))?;
    for (key, expected) in [
        ("component", "skvoz-core"),
        ("version", "5.0.0"),
        ("kind", "shared-library"),
        ("target", target),
        ("profile", "release"),
        ("owning_repository", REPO),
    ] {
        if value[key] != expected {
            return Err(format!("Core manifest mismatch: {key}").into());
        }
    }
    if value["artifact_id"].as_str().is_none_or(str::is_empty)
        || value["abi"] != 1
        || value["features"] != json!(["nats"])
        || value["source"].as_object().map(|v| v.len()) != Some(3)
        || value["source"]["repository"] != REPO
        || value["source"]["revision"] != sha
    {
        return Err("Core ABI/features/source identity mismatch".into());
    }
    for (key, path) in [
        ("sha256", "lib/libskvoz_core.so"),
        ("sdk_manifest_sha256", "sdk/sdk.sha256"),
        ("source_inputs_sha256", "source-inputs.sha256"),
    ] {
        if value[key] != hash(&root.join(path))? {
            return Err(format!("Core manifest digest mismatch: {key}").into());
        }
    }
    if value["source"]["tree_sha256"] != value["source_inputs_sha256"] {
        return Err("Core source tree mismatch".into());
    }
    inventory(&root.join("sdk"), "sdk.sha256", true)?;
    // Source inventory accompanies the binary; source files are intentionally absent.
    inventory(root, "source-inputs.sha256", false)?;
    let elf = output(
        Command::new("readelf")
            .arg("-h")
            .arg(root.join("lib/libskvoz_core.so")),
    )?;
    let machine = if target.starts_with("aarch64") {
        "AArch64"
    } else {
        "Advanced Micro Devices X86-64"
    };
    if !elf.contains("ELF64") || !elf.contains(machine) || !elf.contains("DYN") {
        return Err("Core shared library ELF target mismatch".into());
    }
    if target == "x86_64-unknown-linux-gnu" {
        let dynamic = output(
            Command::new("readelf")
                .arg("-d")
                .arg(root.join("lib/libskvoz_core.so")),
        )?;
        if !dynamic.contains("libc.so.6") {
            return Err("Core GNU libc dependency missing".into());
        }
        let versions = output(
            Command::new("readelf")
                .arg("--version-info")
                .arg(root.join("lib/libskvoz_core.so")),
        )?;
        for token in versions.split_whitespace() {
            if let Some(version) = token.strip_prefix("GLIBC_") {
                let parts: Vec<_> = version
                    .split('.')
                    .map(str::parse::<u32>)
                    .collect::<std::result::Result<_, _>>()?;
                if parts.first() != Some(&2) || parts.get(1).is_none_or(|minor| *minor > 34) {
                    return Err("Core library requires GLIBC newer than 2.34".into());
                }
            }
        }
    }
    let symbols = output(
        Command::new("readelf")
            .args(["--dyn-syms", "--wide"])
            .arg(root.join("lib/libskvoz_core.so")),
    )?;
    if !symbols.contains("skvoz_core_v1_version") {
        return Err("Core C ABI symbol missing".into());
    }
    Ok(value)
}
fn inventory(root: &Path, name: &str, verify_files: bool) -> Result<()> {
    let mut seen = BTreeSet::new();
    for line in fs::read_to_string(root.join(name))?.lines() {
        let (digest, file) = line
            .split_once("  ")
            .or_else(|| line.split_once('\t'))
            .ok_or("invalid inventory line")?;
        if !valid_hash(digest) || !safe_path(file) || !seen.insert(file.to_owned()) {
            return Err("invalid inventory digest/path".into());
        }
        if verify_files
            && (!fs::symlink_metadata(root.join(file))?.is_file()
                || hash(&root.join(file))? != digest)
        {
            return Err("Core SDK inventory mismatch".into());
        }
    }
    if seen.is_empty() {
        return Err("empty Core inventory".into());
    }
    if verify_files {
        let mut actual = BTreeSet::new();
        collect_files(root, root, &mut actual)?;
        actual.remove(name);
        if actual != seen {
            return Err("Core SDK inventory closure mismatch".into());
        }
    }
    Ok(())
}
fn collect_files(root: &Path, dir: &Path, files: &mut BTreeSet<String>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            collect_files(root, &path, files)?;
        } else if metadata.is_file() {
            files.insert(
                path.strip_prefix(root)?
                    .to_str()
                    .ok_or("invalid UTF8 path")?
                    .into(),
            );
        } else {
            return Err("nonregular Core SDK input".into());
        }
    }
    Ok(())
}
fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\n', '\r', '\t', '\\'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn read_json(path: &Path) -> Result<Value> {
    if fs::metadata(path)?.len() > 65536 {
        return Err("manifest exceeds size limit".into());
    }
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
fn hash(path: &Path) -> Result<String> {
    let text = output(Command::new("sha256sum").arg("--").arg(path))?;
    let hash = text.split_whitespace().next().ok_or("missing SHA256")?;
    if !valid_hash(hash) {
        return Err("invalid SHA256".into());
    }
    Ok(hash.into())
}
fn status(command: &mut Command) -> Result<()> {
    if command.status()?.success() {
        Ok(())
    } else {
        Err("external command failed".into())
    }
}
fn output(command: &mut Command) -> Result<String> {
    let result = command.output()?;
    if !result.status.success() {
        return Err("external command failed".into());
    }
    Ok(String::from_utf8(result.stdout)?)
}
fn copy_tree(source: &Path, dest: &Path) -> Result<()> {
    if dest.exists() {
        return Err("destination already exists".into());
    }
    fs::create_dir(dest)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        let target = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&path, &target)?;
        } else if entry.file_type()?.is_file() {
            fs::copy(path, target)?;
        } else {
            return Err("nonregular copy input".into());
        }
    }
    Ok(())
}
fn replace_pair(first: &Path, next_first: &Path, second: &Path, next_second: &Path) -> Result<()> {
    let backup_first = first.with_extension("release-backup");
    let backup_second = second.with_extension("release-backup");
    if backup_first.exists() || backup_second.exists() {
        return Err("unfinished Core refresh backup exists".into());
    }
    fs::rename(first, &backup_first)?;
    let result = (|| -> Result<()> {
        fs::rename(second, &backup_second)?;
        fs::rename(next_first, first)?;
        fs::rename(next_second, second)?;
        Ok(())
    })();
    if result.is_err() {
        if backup_second.exists() {
            if second.exists() {
                fs::remove_dir_all(second)?;
            }
            fs::rename(&backup_second, second)?;
        }
        if first.exists() {
            fs::remove_dir_all(first)?;
        }
        fs::rename(&backup_first, first)?;
    } else {
        fs::remove_dir_all(backup_first)?;
        fs::remove_dir_all(backup_second)?;
    }
    result
}
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Result<Self> {
        Ok(Self(PathBuf::from(
            output(Command::new("mktemp").arg("-d"))?.trim(),
        )))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
