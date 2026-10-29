//! `ADMIN_PASSWORD=… [BIND=127.0.0.1:8080] [DATA_DIR=./data] remote-server`
//! Point Cloudflare Tunnel (or any HTTPS proxy) at BIND.

use std::path::PathBuf;

use remote_server::{App, Config};

#[tokio::main]
async fn main() {
    let admin_password = std::env::var("ADMIN_PASSWORD").unwrap_or_default();
    if admin_password.len() < 12 {
        eprintln!("ADMIN_PASSWORD must be set, at least 12 characters");
        std::process::exit(2);
    }
    let bind = std::env::var("BIND").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let data = PathBuf::from(std::env::var("DATA_DIR").unwrap_or_else(|_| "data".into()));
    if let Err(error) = std::fs::create_dir_all(&data) {
        eprintln!("DATA_DIR {}: {error}", data.display());
        std::process::exit(2);
    }
    let app = App::new(Config { admin_password, data: Some(data) });
    let listener = tokio::net::TcpListener::bind(&bind).await.unwrap_or_else(|error| {
        eprintln!("BIND {bind}: {error}");
        std::process::exit(2);
    });
    eprintln!("listening on {bind}");
    axum::serve(listener, app.router())
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .expect("server");
}
