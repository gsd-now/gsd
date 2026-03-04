#![allow(missing_docs)]

fn main() {
    // version.txt is at the lib root, one level up from artifacts/
    // Binary is at libs/gsd/artifacts/<platform>/gsd
    // So relative to the crate during CI build: ../../libs/gsd/version.txt
    let version_path = "../../libs/gsd/version.txt";

    if let Ok(version) = std::fs::read_to_string(version_path) {
        println!("cargo:rustc-env=GSD_VERSION={}", version.trim());
    } else {
        // No version.txt = development build
        println!("cargo:rustc-env=GSD_VERSION=unknown");
    }
}
