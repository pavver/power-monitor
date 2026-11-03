use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::num::NonZeroU64;
use std::path::Path;
use std::process::Command;
use std::{env, fs};

use power_core::settings::Divider;

/// Set by `sysenv::output` from the ESP-IDF build and read by the
/// `esp_app_desc!` macro, but not declared to the compiler by it.
const ESP_IDF_CFGS: [&str; 10] = [
    "esp_idf_app_build_type_pure_ram_app",
    "esp_idf_app_compile_time_date",
    "esp_idf_app_reproducible_build",
    "esp_idf_idf_target_linux",
    "esp_idf_version_at_least_5_3_2",
    "esp_idf_version_at_least_5_4_0",
    "esp_idf_version_at_least_6_1_0",
    "esp_idf_version_patch_at_least_5_1_7",
    "esp_idf_version_patch_at_least_5_2_3",
    "esp_idf_version_patch_at_least_5_3_2",
];

fn main() {
    embuild::espidf::sysenv::output();
    for cfg in ESP_IDF_CFGS {
        println!("cargo::rustc-check-cfg=cfg({cfg})");
    }
    gzip_page();
    stamp_version();
    check_built_in_divider();
}

/// A wrong built-in divider stops the build, not the device.
fn check_built_in_divider() {
    let [top, bottom] = ["DIVIDER_TOP_OHM", "DIVIDER_BOTTOM_OHM"].map(|name| {
        println!("cargo:rerun-if-env-changed={name}");
        env::var(name).ok()
    });
    if let Err(error) = Divider::built_in(top.as_deref(), bottom.as_deref()) {
        panic!("{error}");
    }
}

/// The page ships gzipped (served with `Content-Encoding: gzip`): a third of
/// its size in flash. Zopfli squeezes harder than `gzip -9`, and every
/// browser unpacks it the same way. It takes seconds, so an unchanged page
/// keeps the last result (this script reruns on every code change).
fn gzip_page() {
    let page = Path::new("ui/index.html");
    println!("cargo:rerun-if-changed={}", page.display());
    let source = fs::read(page).expect("read ui/index.html");
    let out_dir = env::var("OUT_DIR").unwrap();
    let (out, mark) = (Path::new(&out_dir).join("index.html.gz"), Path::new(&out_dir).join("index.html.hash"));
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    let hash = hasher.finish().to_string();
    if out.exists() && fs::read_to_string(&mark).ok().as_deref() == Some(hash.as_str()) {
        return;
    }
    let options = zopfli::Options { iteration_count: NonZeroU64::new(100).unwrap(), ..Default::default() };
    let mut packed = Vec::new();
    zopfli::compress(options, zopfli::Format::Gzip, &source[..], &mut packed).expect("gzip ui/index.html");
    fs::write(out, packed).expect("write index.html.gz");
    fs::write(mark, hash).expect("write index.html.hash");
}

/// The version in the image header (`esp_app_desc!` in main.rs reads
/// CARGO_PKG_VERSION): the commit, with `-dirty` for uncommitted changes.
/// Reruns on a new commit and on any change to the code, so the stamp never
/// lags behind what was built.
fn stamp_version() {
    for path in ["../.git/HEAD", "../.git/index", "src", "ui", "Cargo.toml", "sdkconfig.defaults", "../power-core/src", "../power-core/Cargo.toml"] {
        println!("cargo:rerun-if-changed={path}");
    }
    let described = Command::new("git").args(["describe", "--always", "--dirty"]).output();
    let version = match described {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        _ => "unknown".to_string(),
    };
    println!("cargo:rustc-env=CARGO_PKG_VERSION={version}");
}
