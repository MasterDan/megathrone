//! The Megathrone console web server: the browser-UI shell around
//! `megathrone-core`. Today it serves the built web frontend
//! (`packages/web/dist`) and a health endpoint; the command/event bridge
//! over `/api/*` is the next milestone.

use std::net::SocketAddr;
use std::path::PathBuf;

use axum::routing::get;
use axum::{Json, Router};
use clap::Parser;
use tower_http::services::{ServeDir, ServeFile};

/// Where the built web UI lives. `MEGATHRONE_WEB_DIST` wins (a release
/// install can point anywhere); a dev checkout falls back to the repo path
/// baked in at compile time; a bare cwd gets `./dist`.
fn resolve_web_dist() -> PathBuf {
    if let Some(dir) = std::env::var_os("MEGATHRONE_WEB_DIST") {
        return PathBuf::from(dir);
    }
    let baked = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../packages/web/dist");
    if baked.is_dir() {
        return baked;
    }
    PathBuf::from("dist")
}

#[derive(Parser)]
#[command(
    name = "megathrone-web",
    about = "Megathrone console web server — serves the browser UI",
    version
)]
struct Args {
    /// TCP port to listen on.
    #[arg(long, short, default_value_t = 8420)]
    port: u16,
    /// Interface to bind. Defaults to loopback only.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "server": "megathrone-web",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let addr: SocketAddr = match format!("{}:{}", args.host, args.port).parse() {
        Ok(addr) => addr,
        Err(e) => {
            eprintln!("invalid bind address {}:{} — {e}", args.host, args.port);
            std::process::exit(2);
        }
    };

    let dist = resolve_web_dist();
    let index = dist.join("index.html");
    let dist_ready = index.is_file();

    let app = Router::new()
        .route("/api/health", get(health))
        .fallback_service(ServeDir::new(&dist).fallback(ServeFile::new(index)));

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("failed to bind {addr} — {e}");
            std::process::exit(1);
        }
    };

    println!("megathrone-web listening on http://{addr}/");
    if !dist_ready {
        println!(
            "warning: web UI not found at {} — serving a placeholder",
            dist.display()
        );
    }

    axum::serve(listener, app).await.expect("server error");
}
