#![allow(missing_docs)]

fn main() {
    // version.txt is at the lib root, one level up from artifacts/
    // Binary is at libs/agent_pool/artifacts/<platform>/agent_pool
    // So relative to the crate during CI build: ../../libs/agent_pool/version.txt
    let version_path = "../../libs/agent_pool/version.txt";

    if let Ok(version) = std::fs::read_to_string(version_path) {
        println!("cargo:rustc-env=AGENT_POOL_VERSION={}", version.trim());
    } else {
        // No version.txt = development build
        println!("cargo:rustc-env=AGENT_POOL_VERSION=unknown");
    }
}
