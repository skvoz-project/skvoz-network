use std::{env, fs, path::PathBuf, process::Command};
fn main() {
    println!("cargo:rerun-if-env-changed=SKVOZ_CORE_ARTIFACT");
    let directory = PathBuf::from(env::var_os("SKVOZ_CORE_ARTIFACT").expect(
        "SKVOZ_CORE_ARTIFACT must name a prepared Core artifact; source fallback is forbidden",
    ));
    let manifest = directory.join("artifact.json");
    println!("cargo:rerun-if-changed={}", manifest.display());
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).expect("Core artifact manifest missing"))
            .expect("invalid Core artifact manifest");
    assert_eq!(value["component"], "skvoz-core", "wrong Core component");
    assert_eq!(value["version"], "5.0.0", "wrong Core version");
    assert_eq!(value["abi"], 1, "unsupported Core ABI");
    assert_eq!(
        value["target"],
        env::var("TARGET").unwrap(),
        "Core target mismatch"
    );
    let library = directory.join("lib/libskvoz_core.so");
    println!("cargo:rerun-if-changed={}", library.display());
    let digest = Command::new("sha256sum")
        .arg(&library)
        .output()
        .expect("sha256sum is required to verify Core bytes");
    assert!(digest.status.success(), "cannot hash Core library");
    let hash = String::from_utf8(digest.stdout).expect("invalid sha256sum output");
    assert_eq!(
        Some(hash.split_whitespace().next().unwrap()),
        value["sha256"].as_str(),
        "Core artifact digest mismatch"
    );
    println!(
        "cargo:rustc-link-search=native={}",
        directory.join("lib").display()
    );
    println!("cargo:rustc-link-lib=dylib=skvoz_core");
    // Final executable owners set their package-specific loader path.
}
