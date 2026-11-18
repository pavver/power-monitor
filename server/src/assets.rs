//! The admin Leptos CSR app and its static files, built into the binary.

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

impl Asset {
    fn response(&'static self) -> Response {
        let headers = [
            (header::CONTENT_TYPE, self.content_type),
            (header::CACHE_CONTROL, self.cache),
        ];
        (headers, self.body).into_response()
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
