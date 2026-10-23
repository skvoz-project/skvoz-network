//! Export the two production source closures without building either consumer.
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn main() {
    if let Err(error) = run() {
        eprintln!("SDK export failed: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [root, profile, output] = args.as_slice() else {
        return Err(
            "Usage: export-sdk REPOSITORY desktop-host|portable-supplier OUTPUT_DIRECTORY".into(),
        );
    };
    if !matches!(profile.as_str(), "desktop-host" | "portable-supplier") {
        return Err("unsupported source SDK profile".into());
    }
    let root = Path::new(root).canonicalize()?;
    let source = root.join("src");
    let output = Path::new(output);
    if output.exists() {
        return Err("output directory already exists".into());
    }
    let scratch = Scratch(PathBuf::from(
        text(Command::new("mktemp").arg("-d"))?.trim(),
    ));
    let sdk = scratch.0.join("sdk");
    fs::create_dir(&sdk)?;
    copy(&root.join("LICENSE"), &sdk.join("LICENSE"))?;
    copy(&source.join("Cargo.lock"), &sdk.join("Cargo.lock"))?;
    let packages = if profile == "desktop-host" {
        &["contracts", "client", "native"][..]
    } else {
        &["contracts", "native"][..]
    };
    for package in packages {
        let target = sdk.join(package);
        fs::create_dir(&target)?;
        copy_tree(&source.join(package).join("src"), &target.join("src"))?;
        let mut manifest = fs::read_to_string(source.join(package).join("Cargo.toml"))?;
        if profile == "desktop-host" {
            manifest = manifest.split("[[test]]").next().unwrap().to_owned();
            for (before, after) in [
                ("edition.workspace = true", "edition = \"2024\""),
                ("rust-version.workspace = true", "rust-version = \"1.92\""),
                ("license.workspace = true", "license = \"MIT\""),
                (
                    "[lints]\nworkspace = true",
                    "[lints.rust]\nunsafe_code = \"forbid\"",
                ),
            ] {
                manifest = manifest.replace(before, after);
            }
        }
        fs::write(target.join("Cargo.toml"), manifest)?;
    }
    fs::create_dir_all(sdk.join("vendor/skvoz-core"))?;
    if profile == "desktop-host" {
        copy_tree(
            &source.join("vendor/skvoz-core/contracts"),
            &sdk.join("vendor/skvoz-core/contracts"),
        )?;
        copy(
            &source.join("vendor/skvoz-core/LICENSE"),
            &sdk.join("vendor/skvoz-core/LICENSE"),
        )?;
    } else {
        copy_tree(&source.join("network/src"), &sdk.join("src"))?;
        fs::remove_dir_all(sdk.join("src/bin"))?;
        copy(&source.join("network/build.rs"), &sdk.join("build.rs"))?;
        let workspace = fs::read_to_string(source.join("Cargo.toml"))?;
        let workspace = workspace
            .lines()
            .map(|line| {
                if line.starts_with("members = ") {
                    "members = [\".\", \"contracts\", \"native\"]"
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let package = fs::read_to_string(source.join("network/Cargo.toml"))?;
        let package = package.split("[dev-dependencies]").next().unwrap();
        let before = package
            .find("[[bin]]")
            .ok_or("runtime binary declaration missing")?;
        let after = package[before..]
            .find("[dependencies]")
            .ok_or("runtime dependencies missing")?
            + before;
        let package = format!("{}{}", &package[..before], &package[after..])
            .replace("path = \"../", "path = \"");
        fs::write(sdk.join("Cargo.toml"), format!("{workspace}\n\n{package}"))?;
        // The SDK's runtime package is flat; its pin path follows that layout.
        let build = fs::read_to_string(sdk.join("build.rs"))?
            .replace("../dependencies/core/", "dependencies/core/");
        fs::write(sdk.join("build.rs"), build)?;
        fs::remove_dir(sdk.join("vendor/skvoz-core"))?;
        copy_tree(
            &source.join("vendor/skvoz-core"),
            &sdk.join("vendor/skvoz-core"),
        )?;
        copy_tree(
            &source.join("vendor/async-nats/src"),
            &sdk.join("vendor/async-nats/src"),
        )?;
        for file in ["Cargo.toml", "README.md", "LICENSE", "PATCHES.md"] {
            copy(
                &source.join("vendor/async-nats").join(file),
                &sdk.join("vendor/async-nats").join(file),
            )?;
        }
        for target in ["aarch64-linux-android", "x86_64-linux-android"] {
            for prefix in ["dependencies/core", "dependencies/core/descriptors"] {
                let path = format!("{prefix}/{target}.json");
                copy(&source.join(&path), &sdk.join(path))?;
            }
        }
    }
    if profile == "portable-supplier" {
        // Cargo removes owning-workspace/test edges while keeping locked
        // versions for the exported production workspace. Consumers use --locked.
        checked(
            Command::new("cargo")
                .args(["metadata", "--format-version", "1"])
                .arg("--manifest-path")
                .arg(sdk.join("Cargo.toml")),
        )?;
    }
    let detail = if profile == "desktop-host" {
        "Профиль `desktop-host` содержит contracts/client/native и Core contracts. Engine, NATS и TLS adapter остаются у поставщика готовых runtime/helper outputs."
    } else {
        "Профиль `portable-supplier` содержит runtime/contracts/native, patched async-nats, полный Core SDK и Android pins/descriptors. Android JNI supplier передаёт готовые Core libraries через `SKVOZ_CORE_ARTIFACT`."
    };
    fs::write(
        sdk.join("README.md"),
        format!(
            "# Network source SDK\n\n[skvoz-network](https://github.com/skvoz-project/skvoz-network) 0.7.0, Core 5.0.0.\n\n{detail}\n\nСостав и зависимости закреплены в `sdk.json`, `source-inputs.sha256` и `Cargo.lock`. Тесты и инструменты остаются в owning репозитории. Готовые outputs используются по [контракту артефактов](https://github.com/skvoz-project/skvoz-network/blob/main/docs/artifacts.md). Network и Core сохраняют MIT лицензии; patched async-nats — Apache-2.0.\n"
        ),
    )?;
    let mut files = BTreeMap::new();
    inventory(&sdk, &sdk, &mut files)?;
    let inventory: String = files
        .iter()
        .map(|(name, hash)| format!("{hash}\t{name}\n"))
        .collect();
    fs::write(sdk.join("source-inputs.sha256"), inventory)?;
    let head =
        text(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["rev-parse", "--verify", "HEAD"]),
        )?;
    let clean = text(
        Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["status", "--porcelain"]),
    )?
    .is_empty();
    let revision = clean.then_some(head.trim());
    if let Ok(expected) = std::env::var("GITHUB_SHA")
        && revision != Some(expected.as_str())
    {
        return Err("CI source must be the clean owning commit".into());
    }
    let manifest = json!({"schema":1,"component":"skvoz-network-sdk","version":"0.7.0","kind":"source-sdk","profile":profile,
        "tuple":{"api":1,"network":6,"core":"5.0.0","helper":"0.6.0","core_abi":1,"network_abi":1},
        "source":{"repository":"https://github.com/skvoz-project/skvoz-network","revision":revision,"tree_sha256":hash(&sdk.join("source-inputs.sha256"))?},
        "dependencies":[{"component":"skvoz-core-source-sdk","version":"5.0.0","manifest_sha256":hash(&source.join("vendor/skvoz-core/sdk.sha256"))?}],
        "qualification":"source candidate; compilation and platform checks pending"});
    fs::write(sdk.join("sdk.json"), serde_json::to_vec_pretty(&manifest)?)?;
    files.insert("source-inputs.sha256".into(), String::new());
    files.insert("sdk.json".into(), String::new());
    let list = scratch.0.join("files");
    fs::write(
        &list,
        files
            .keys()
            .map(|path| format!("{path}\n"))
            .collect::<String>(),
    )?;
    fs::create_dir_all(output)?;
    let output = output.canonicalize()?;
    let archive = output.join(format!("skvoz-network-sdk-0.7.0-{profile}.tar.gz"));
    checked(
        Command::new("tar")
            .arg("-C")
            .arg(&sdk)
            .args(["--no-recursion", "--verbatim-files-from", "-T"])
            .arg(list)
            .args([
                "--sort=name",
                "--mtime=UTC 2026-10-19",
                "--owner=0",
                "--group=0",
                "--numeric-owner",
                "--mode=0644",
                "-czf",
            ])
            .arg(&archive),
    )?;
    copy(&sdk.join("sdk.json"), &output.join("sdk.json"))?;
    fs::write(
        output.join("archive.sha256"),
        format!(
            "{}  {}\n",
            hash(&archive)?,
            archive.file_name().unwrap().to_str().unwrap()
        ),
    )?;
    println!("{}", archive.display());
    Ok(())
}
fn inventory(root: &Path, dir: &Path, files: &mut BTreeMap<String, String>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            inventory(root, &path, files)?;
        } else if metadata.is_file() {
            let name = path
                .strip_prefix(root)?
                .to_str()
                .ok_or("invalid source path")?;
            if name.contains(['\n', '\r', '\t']) {
                return Err("invalid source path".into());
            }
            files.insert(name.into(), hash(&path)?);
        } else {
            return Err("nonregular SDK input".into());
        }
    }
    Ok(())
}
fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target.join(entry.file_name()))?;
        } else if entry.file_type()?.is_file() {
            copy(&entry.path(), &target.join(entry.file_name()))?;
        } else {
            return Err("nonregular SDK input".into());
        }
    }
    Ok(())
}
fn copy(source: &Path, target: &Path) -> io::Result<()> {
    fs::create_dir_all(target.parent().unwrap())?;
    fs::copy(source, target)?;
    Ok(())
}
fn hash(path: &Path) -> Result<String> {
    Ok(text(Command::new("sha256sum").arg("--").arg(path))?
        .split_whitespace()
        .next()
        .ok_or("missing SHA256")?
        .into())
}
fn text(command: &mut Command) -> Result<String> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(format!(
            "command failed: {command:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?)
}
fn checked(command: &mut Command) -> Result<()> {
    text(command).map(|_| ())
}
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
