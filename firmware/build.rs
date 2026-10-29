use std::num::NonZeroU64;
use std::path::Path;
use std::{env, fs};

fn main() {
    embuild::espidf::sysenv::output();
    gzip_page();
}

/// The page ships gzipped (served with `Content-Encoding: gzip`): a third of
/// its size in flash. Zopfli squeezes harder than `gzip -9`, and every
/// browser unpacks it the same way.
fn gzip_page() {
    let page = Path::new("ui/index.html");
    println!("cargo:rerun-if-changed={}", page.display());
    let source = fs::read(page).expect("read ui/index.html");
    let options = zopfli::Options { iteration_count: NonZeroU64::new(100).unwrap(), ..Default::default() };
    let mut packed = Vec::new();
    zopfli::compress(options, zopfli::Format::Gzip, &source[..], &mut packed).expect("gzip ui/index.html");
    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("index.html.gz");
    fs::write(out, packed).expect("write index.html.gz");
}
