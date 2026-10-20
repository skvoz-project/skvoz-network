use std::{env, fs, path::PathBuf, process::Command};
fn main() {
    let target = env::var("TARGET").expect("Cargo target missing");
    let lock = PathBuf::from(format!("dependencies/core/{target}.json"));
    println!("cargo:rerun-if-changed={}", lock.display());
    println!("cargo:rerun-if-env-changed=SKVOZ_CORE_ARTIFACT");
    let expected: serde_json::Value = serde_json::from_slice(
        &fs::read(&lock).expect("no qualified pinned Core library for this target"),
    )
    .expect("invalid Core dependency lock");
    let artifact = PathBuf::from(
        env::var_os("SKVOZ_CORE_ARTIFACT")
            .expect("prepare the pinned Core artifact and set SKVOZ_CORE_ARTIFACT"),
    );
    let manifest = artifact.join("artifact.json");
    println!("cargo:rerun-if-changed={}", manifest.display());
    let actual: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).expect("Core artifact manifest missing"))
            .expect("invalid Core artifact manifest");
    for key in [
        "component",
        "version",
        "abi",
        "target",
        "artifact_id",
        "sha256",
        "sdk_manifest_sha256",
        "source_inputs_sha256",
    ] {
        assert_eq!(
            actual[key], expected[key],
            "pinned Core dependency mismatch: {key}"
        );
    }
    if target.ends_with("-linux-android") {
        let digest = Command::new("sha256sum")
            .arg(&manifest)
            .output()
            .expect("sha256sum is required to verify the Android Core descriptor");
        assert!(
            digest.status.success(),
            "cannot hash the Android Core descriptor"
        );
        let digest = String::from_utf8(digest.stdout).expect("invalid SHA256 output");
        assert_eq!(
            digest.split_whitespace().next(),
            expected["artifact_manifest_sha256"].as_str(),
            "pinned Android Core descriptor mismatch"
        );
        assert_eq!(actual["kind"], "shared-library");
        for key in [
            "android_abi",
            "minimum_api",
            "page_alignment",
            "ndk_revision",
            "elf_machine",
        ] {
            assert_eq!(
                actual["android"][key], expected["android"][key],
                "pinned Android Core profile mismatch: {key}"
            );
        }
    }
    // The supplier binding checks the actual library digest before linking.
    // Product archives preserve bin/ beside the verified native dependency lib/.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../lib");
    }
}
