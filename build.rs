fn main() {
    let version = display_version();
    println!("cargo:rustc-env=RC_VERSION={version}");
    println!("cargo:rustc-env=RC_TITLE=Rust Commander {version}");
    println!("cargo:rerun-if-env-changed=RC_PKG_VERSION");
    println!("cargo:rerun-if-changed=Cargo.toml");
}

fn display_version() -> String {
    // Packagers can pin the release version without editing Cargo.toml.
    if let Ok(v) = std::env::var("RC_PKG_VERSION") {
        let v = v.trim();
        if !v.is_empty() {
            return v.to_string();
        }
    }
    std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.2.1".into())
}
