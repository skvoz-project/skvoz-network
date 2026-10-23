//! Release transport is mocked; these checks do not qualify native libraries.
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};
const SHA: &str = "0123456789012345678901234567890123456789";
const TARGETS: [&str; 3] = [
    "x86_64-unknown-linux-gnu",
    "aarch64-linux-android",
    "x86_64-linux-android",
];
fn hash(path: &Path) -> String {
    String::from_utf8(Command::new("sha256sum").arg(path).output().unwrap().stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .into()
}
fn script(path: &Path, source: &str) {
    fs::write(path, source).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
struct Fixture {
    root: PathBuf,
    repo: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(
            String::from_utf8(Command::new("mktemp").arg("-d").output().unwrap().stdout)
                .unwrap()
                .trim(),
        );
        let repo = root.join("repo");
        fs::create_dir_all(repo.join("dependencies/core/descriptors")).unwrap();
        fs::create_dir_all(repo.join("vendor/skvoz-core")).unwrap();
        fs::write(repo.join("vendor/skvoz-core/old"), "unchanged").unwrap();
        fs::create_dir(root.join("release")).unwrap();
        fs::create_dir(root.join("bin")).unwrap();
        // gh arguments: release download TAG --repo REPO --pattern ASSET --dir DIR.
        script(
            &root.join("bin/gh"),
            &format!(
                "#!/bin/sh\nset -eu\ncase $1 in\napi) printf '%s\\n' '{}' ;;\nrelease) cp \"$MOCK_RELEASE/$7\" \"${{9}}/\" ;;\n*) exit 1 ;;\nesac\n",
                SHA
            ),
        );
        script(
            &root.join("bin/readelf"),
            "#!/bin/sh\n[ \"${MOCK_BAD_ELF:-}\" != 1 ] || exit 1\nprintf '%s\\n' 'ELF64 DYN Advanced Micro Devices X86-64 AArch64 skvoz_core_v1_version libc.so.6 GLIBC_2.34'\n",
        );
        let mut sums = String::new();
        for target in TARGETS {
            let artifact = root.join(target);
            fs::create_dir_all(artifact.join("lib")).unwrap();
            fs::create_dir_all(artifact.join("sdk")).unwrap();
            fs::write(artifact.join("sdk/LICENSE"), "fixture MIT").unwrap();
            fs::write(
                artifact.join("sdk/sdk.sha256"),
                format!("{}  LICENSE\n", hash(&artifact.join("sdk/LICENSE"))),
            )
            .unwrap();
            fs::write(
                artifact.join("source-inputs.sha256"),
                format!("{}  Cargo.toml\n", "a".repeat(64)),
            )
            .unwrap();
            fs::write(artifact.join("lib/libskvoz_core.so"), target).unwrap();
            let mut manifest = json!({"component":"skvoz-core","version":"5.0.0","abi":1,"kind":"shared-library","target":target,"profile":"release","features":["nats"],"owning_repository":"https://github.com/skvoz-project/skvoz-core","artifact_id":format!("fixture-{target}"),"sha256":hash(&artifact.join("lib/libskvoz_core.so")),"sdk_manifest_sha256":hash(&artifact.join("sdk/sdk.sha256")),"source_inputs_sha256":hash(&artifact.join("source-inputs.sha256")),"source":{"repository":"https://github.com/skvoz-project/skvoz-core","revision":SHA,"tree_sha256":hash(&artifact.join("source-inputs.sha256"))}});
            let profile = json!({"android_abi":if target.starts_with("aarch64") {"arm64-v8a"} else {"x86_64"},"minimum_api":31,"page_alignment":16384,"ndk_revision":"30.0.16248370","elf_machine":if target.starts_with("aarch64") {"AArch64"} else {"Advanced Micro Devices X86-64"}});
            if target.ends_with("-android") {
                manifest["android"] = profile.clone();
                // Supplier descriptor includes provenance in addition to the stable pin.
                manifest["android"]["library"] = json!("lib/libskvoz_core.so");
                manifest["android"]["build_environment"] =
                    json!({"clang":"fixture", "profile":"release"});
            }
            fs::write(
                artifact.join("artifact.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            fs::write(repo.join(format!("dependencies/core/{target}.json")), serde_json::to_vec(&json!({"android":profile,"supplier":"https://github.com/skvoz-project/skvoz-core","manifest":"artifact.json","library":"lib/libskvoz_core.so"})).unwrap()).unwrap();
            let name = format!("skvoz-core-5.0.0-{target}.tar.gz");
            assert!(
                Command::new("tar")
                    .arg("-czf")
                    .arg(root.join("release").join(&name))
                    .arg("-C")
                    .arg(artifact)
                    .arg(".")
                    .status()
                    .unwrap()
                    .success()
            );
            sums.push_str(&format!(
                "{}  {name}\n",
                hash(&root.join("release").join(&name))
            ));
        }
        fs::write(root.join("release/SHA256SUMS"), sums).unwrap();
        Self { root, repo }
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_core-release"));
        command
            .current_dir(&self.repo)
            .args(args)
            .env("MOCK_RELEASE", self.root.join("release"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.root.join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            );
        command
    }
    fn run(&self, args: &[&str]) -> bool {
        self.command(args).status().unwrap().success()
    }
    fn tag(&self) -> String {
        format!("core-v5.0.0-{SHA}")
    }
    fn repack(&self, target: &str) {
        let name = format!("skvoz-core-5.0.0-{target}.tar.gz");
        let archive = self.root.join("release").join(&name);
        assert!(
            Command::new("tar")
                .arg("-czf")
                .arg(&archive)
                .arg("-C")
                .arg(self.root.join(target))
                .arg(".")
                .status()
                .unwrap()
                .success()
        );
        let sums = fs::read_to_string(self.root.join("release/SHA256SUMS")).unwrap();
        let sums: String = sums
            .lines()
            .map(|line| {
                if line.ends_with(&name) {
                    format!("{}  {name}\n", hash(&archive))
                } else {
                    format!("{line}\n")
                }
            })
            .collect();
        fs::write(self.root.join("release/SHA256SUMS"), sums).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn refresh_fetch_and_failures_preserve_pins() {
    let f = Fixture::new();
    assert!(!f.run(&["refresh", "latest"]));
    assert!(f.run(&["refresh", &f.tag()]));
    let pin_path = f
        .repo
        .join(format!("dependencies/core/{}.json", TARGETS[0]));
    let pin_before = fs::read(&pin_path).unwrap();
    let pin: Value = serde_json::from_slice(&pin_before).unwrap();
    assert_eq!(pin["source_revision"], SHA);
    assert!(f.repo.join("vendor/skvoz-core/sdk.sha256").is_file());
    assert!(f.run(&[
        "fetch",
        TARGETS[0],
        f.root.join("fetched").to_str().unwrap()
    ]));
    let linux_archive = f
        .root
        .join("release")
        .join(format!("skvoz-core-5.0.0-{}.tar.gz", TARGETS[0]));
    let linux_bytes = fs::read(&linux_archive).unwrap();
    fs::write(&linux_archive, b"corrupted archive").unwrap();
    assert!(!f.run(&[
        "fetch",
        TARGETS[0],
        f.root.join("corrupted").to_str().unwrap()
    ]));
    assert!(!f.root.join("corrupted").exists());
    fs::write(linux_archive, linux_bytes).unwrap();
    let artifact = f.root.join(TARGETS[1]).join("artifact.json");
    let original = fs::read(&artifact).unwrap();
    let mut value: Value = serde_json::from_slice(&original).unwrap();
    value["source"]["revision"] = json!("0".repeat(40));
    fs::write(&artifact, serde_json::to_vec(&value).unwrap()).unwrap();
    f.repack(TARGETS[1]);
    assert!(!f.run(&["refresh", &f.tag()]));
    assert_eq!(fs::read(&pin_path).unwrap(), pin_before);
    assert!(
        !f.command(&[
            "fetch",
            TARGETS[0],
            f.root.join("bad-elf").to_str().unwrap()
        ])
        .env("MOCK_BAD_ELF", "1")
        .status()
        .unwrap()
        .success()
    );
    fs::write(&artifact, &original).unwrap();
    for (key, wrong) in [
        ("abi", json!(2)),
        ("version", json!("5.1.0")),
        ("target", json!(TARGETS[0])),
        ("sdk_manifest_sha256", json!("0".repeat(64))),
        ("sha256", json!("0".repeat(64))),
    ] {
        let mut changed: Value = serde_json::from_slice(&original).unwrap();
        changed[key] = wrong;
        fs::write(&artifact, serde_json::to_vec(&changed).unwrap()).unwrap();
        f.repack(TARGETS[1]);
        assert!(!f.run(&["refresh", &f.tag()]));
        assert_eq!(fs::read(&pin_path).unwrap(), pin_before);
    }
    for key in [
        "android_abi",
        "minimum_api",
        "page_alignment",
        "ndk_revision",
        "elf_machine",
    ] {
        let mut changed: Value = serde_json::from_slice(&original).unwrap();
        changed["android"][key] = json!("wrong");
        fs::write(&artifact, serde_json::to_vec(&changed).unwrap()).unwrap();
        f.repack(TARGETS[1]);
        assert!(!f.run(&["refresh", &f.tag()]));
        assert_eq!(fs::read(&pin_path).unwrap(), pin_before);
    }
    fs::write(&artifact, original).unwrap();
    f.repack(TARGETS[1]);
    fs::write(f.root.join(TARGETS[2]).join("sdk/unlisted"), "unexpected").unwrap();
    f.repack(TARGETS[2]);
    assert!(!f.run(&["refresh", &f.tag()]));
    assert_eq!(fs::read(&pin_path).unwrap(), pin_before);
    fs::write(f.root.join("release/SHA256SUMS"), "invalid\n").unwrap();
    assert!(!f.run(&[
        "fetch",
        TARGETS[0],
        f.root.join("bad-digest").to_str().unwrap()
    ]));
    assert!(!f.root.join("bad-digest").exists());
}

#[test]
fn public_transport_preserves_auth_path_and_rejects_invalid_responses() {
    let f = Fixture::new();
    script(&f.root.join("bin/gh"), "#!/bin/sh\nexit 77\n");
    script(
        &f.root.join("bin/curl"),
        &format!(
            r#"#!/bin/sh
set -eu
[ "$1" = --disable ]
[ "$2" = --proto ] && [ "$3" = =https ]
[ "$4" = --proto-redir ] && [ "$5" = =https ]
[ "$6" = --fail ] && [ "$7" = --location ]
[ "$8" = --silent ] && [ "$9" = --show-error ]
shift 9
[ "$1" = --connect-timeout ] && [ "$2" = 10 ]
[ "$3" = --max-time ] && [ "$4" = 115 ]
url=$5
shift 5
[ "${{MOCK_PUBLIC_HTTP_ERROR:-0}}" = 0 ] || exit 22
case "$url" in
  https://api.github.com/repos/skvoz-project/skvoz-core/git/ref/tags/core-v5.0.0-{SHA})
    [ "$#" = 0 ]
    case "${{MOCK_PUBLIC_RESPONSE:-valid}}" in
      invalid-json) printf '%s' '{{' ;;
      wrong-ref) printf '%s' '{{"object":{{"sha":"wrong"}}}}' ;;
      valid) printf '%s' '{{"object":{{"sha":"{SHA}"}}}}' ;;
      *) exit 1 ;;
    esac ;;
  https://api.github.com/repos/skvoz-project/skvoz-core/commits/{SHA})
    [ "$#" = 0 ]
    printf '%s' '{{"sha":"{SHA}"}}' ;;
  https://github.com/skvoz-project/skvoz-core/releases/download/core-v5.0.0-{SHA}/*)
    [ "$#" = 2 ] && [ "$1" = --output ]
    cp "$MOCK_RELEASE/${{url##*/}}" "$2" ;;
  *) exit 1 ;;
esac
"#
        ),
    );
    assert!(!f.run(&["refresh", &f.tag()]));
    assert!(f.run(&["--public", "refresh", &f.tag()]));
    assert!(f.run(&[
        "--public",
        "fetch",
        TARGETS[0],
        f.root.join("public-fetched").to_str().unwrap()
    ]));
    let pin_path = f
        .repo
        .join(format!("dependencies/core/{}.json", TARGETS[0]));
    let before = fs::read(&pin_path).unwrap();
    for (key, value) in [
        ("MOCK_PUBLIC_RESPONSE", "wrong-ref"),
        ("MOCK_PUBLIC_RESPONSE", "invalid-json"),
        ("MOCK_PUBLIC_HTTP_ERROR", "1"),
    ] {
        assert!(
            !f.command(&["--public", "refresh", &f.tag()])
                .env(key, value)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(fs::read(&pin_path).unwrap(), before);
    }
}
