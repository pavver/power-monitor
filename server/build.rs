use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()));
    let admin_dir = manifest_dir.join("admin");

    println!("cargo::rerun-if-changed=admin/src");
    println!("cargo::rerun-if-changed=admin/index.html");
    println!("cargo::rerun-if-changed=admin/admin.css");
    println!("cargo::rerun-if-changed=admin/Trunk.toml");
    println!("cargo::rerun-if-changed=admin/Cargo.toml");
    println!("cargo::rerun-if-changed=admin/Cargo.lock");
    println!("cargo::rerun-if-changed=admin/assets");

    // Fail with a clear message if trunk is missing
    let trunk_version = Command::new("trunk").arg("--version").output();
    let has_trunk = match trunk_version {
        Ok(out) => out.status.success(),
        Err(_) => false,
    };

    if !has_trunk {
        panic!(
            "\n\n========================================================================\n\
             ERROR: 'trunk' is required to build server/admin, but was not found in PATH.\n\
             Please install trunk: cargo install trunk\n\
             ========================================================================\n\n"
        );
    }

    // Use a dedicated target directory so trunk's cargo invocation never deadlocks
    // on the outer cargo's target directory lock.
    // Also remove CARGO_ENCODED_RUSTFLAGS / RUSTFLAGS which may contain host-only
    // flags (such as mold linker flags) that break wasm32-unknown-unknown.
    let target_dir = admin_dir.join("target");
    let status = Command::new("trunk")
        .current_dir(&admin_dir)
        .arg("build")
        .arg("--release")
        .env("CARGO_TARGET_DIR", &target_dir)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTFLAGS")
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => panic!("trunk build failed with exit code: {}", s),
        Err(e) => panic!("Failed to run trunk: {}", e),
    }
}
