//! The admin Leptos CSR app and its static files, built into the binary.

use std::sync::LazyLock;

use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::App;

/// A file of the admin app, served at each of `paths`.
struct Asset {
    paths: &'static [&'static str],
    content_type: &'static str,
    cache: &'static str,
    body: &'static [u8],
}

const CODE: &str = "public, max-age=3600";
const FONT: &str = "public, max-age=86400";

static ASSETS: [Asset; 6] = [
    Asset {
        paths: &["/admin", "/admin/", "/admin/index.html"],
        content_type: "text/html; charset=utf-8",
        // Always the page of the running server, so it never asks for gone files.
        cache: "no-store",
        body: include_bytes!("../admin/dist/index.html"),
    },
    Asset {
        paths: &["/admin/admin.js"],
        content_type: "application/javascript",
        cache: CODE,
        body: include_bytes!("../admin/dist/admin.js"),
    },
    Asset {
        paths: &["/admin/admin_bg.wasm", "/admin/admin.wasm"],
        content_type: "application/wasm",
        cache: CODE,
        body: include_bytes!("../admin/dist/admin_bg.wasm"),
    },
    Asset {
        paths: &["/admin/admin.css"],
        content_type: "text/css; charset=utf-8",
        cache: CODE,
        body: include_bytes!("../admin/dist/admin.css"),
    },
    Asset {
        paths: &["/admin/assets/fonts/CascadiaCode.ttf"],
        content_type: "font/ttf",
        cache: FONT,
        body: include_bytes!("../admin/dist/assets/fonts/CascadiaCode.ttf"),
    },
    Asset {
        paths: &["/admin/assets/fonts/CascadiaCode-OFL.txt"],
        content_type: "text/plain; charset=utf-8",
        cache: FONT,
        body: include_bytes!("../admin/dist/assets/fonts/CascadiaCode-OFL.txt"),
    },
];

/// The code files the page asks for. Cloudflare keeps them for hours, so
/// the page names each with a version of its content (`?v=`): a new build's
/// page never meets an old file, which its `integrity` check would refuse.
const VERSIONED: [&str; 3] = [
    "/admin/admin.js",
    "/admin/admin_bg.wasm",
    "/admin/admin.css",
];

/// The page with every `VERSIONED` file named by its version.
static PAGE: LazyLock<String> = LazyLock::new(|| {
    let page = String::from_utf8_lossy(ASSETS[0].body).into_owned();
    VERSIONED.iter().fold(page, |page, path| {
        let body = ASSETS
            .iter()
            .find(|asset| asset.paths.contains(path))
            .expect("a versioned file is an asset")
            .body;
        let versioned = format!("{path}?v={:016x}", fnv1a(body));
        page.replace(&format!("\"{path}\""), &format!("\"{versioned}\""))
            .replace(&format!("'{path}'"), &format!("'{versioned}'"))
    })
});

/// FNV-1a: a version that changes with the content, not a security check.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

impl Asset {
    fn response(&'static self) -> Response {
        let headers = [
            (header::CONTENT_TYPE, self.content_type),
            (header::CACHE_CONTROL, self.cache),
        ];
        let body = if std::ptr::eq(self, &ASSETS[0]) {
            PAGE.as_bytes()
        } else {
            self.body
        };
        (headers, body).into_response()
    }
}

/// `router` with a route for every path of every asset.
pub(crate) fn routes(router: Router<App>) -> Router<App> {
    ASSETS.iter().fold(router, |router, asset| {
        asset.paths.iter().fold(router, |router, path| {
            router.route(path, get(move || async move { asset.response() }))
        })
    })
}
