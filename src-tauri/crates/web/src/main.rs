//! The Megathrone console web server: the browser-UI shell around
//! `megathrone-core`. It performs the same startup the Tauri app performs
//! in its setup hook (database + seeds + the core environment + the
//! scheduler + crash recovery), then serves the built web frontend
//! (`packages/web/dist`), the command bridge `POST /api/<command>` and the
//! event stream `GET /api/events` (SSE) the browser shell's
//! `PlatformApi` talks to.

mod api;
mod events;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use tokio::sync::{broadcast, Notify};
use tower_http::services::{ServeDir, ServeFile};

use megathrone_core::env;
use megathrone_core::{connection, db, discovery, dpi, scheduler, sites, AppState};

use crate::events::WebSink;

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
    /// Data directory (database, session marker). Defaults to the OS data
    /// dir — deliberately separate from the desktop app's, so both can run.
    #[arg(long)]
    data_dir: Option<PathBuf>,
}

fn default_data_dir() -> Result<PathBuf, String> {
    dirs::data_dir()
        .map(|dir| dir.join("megathrone-web"))
        .ok_or_else(|| "cannot resolve the OS data directory — pass --data-dir".to_string())
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

    // the same startup the Tauri shell runs in its setup hook: open (and
    // migrate) the database, seed the default catalogs, install the core
    // environment — here with the SSE sink instead of the app's event bus
    // — arm the scheduler and recover from an unclean previous death
    let data_dir = match args.data_dir.map_or_else(default_data_dir, Ok) {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        eprintln!(
            "failed to create the data directory {} — {e}",
            data_dir.display()
        );
        std::process::exit(1);
    }
    let database = match db::open(&data_dir.join("megathrone.db"), &db::Migrations::embedded()) {
        Ok(database) => database,
        Err(e) => {
            eprintln!("failed to initialize the profiles database: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = dpi::seed_default_strategies(&database) {
        eprintln!("failed to seed the default DPI strategies: {e}");
        std::process::exit(1);
    }
    if let Err(e) = sites::seed_default_sites(&database) {
        eprintln!("failed to seed the default test sites: {e}");
        std::process::exit(1);
    }
    if let Err(e) = discovery::seed_discovery_sources(&database) {
        eprintln!("failed to seed the discovery sources: {e}");
        std::process::exit(1);
    }
    let (events_tx, _) = broadcast::channel::<events::EventFrame>(events::CHANNEL_CAPACITY);
    let notify = Arc::new(Notify::new());
    env::install(
        Arc::new(AppState::new(database, Arc::clone(&notify))),
        Arc::new(WebSink::new(events_tx.clone())),
        Some(data_dir.clone()),
    );
    scheduler::spawn(notify);
    connection::recover();

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/events", get(events::sse))
        .route("/api/{command}", post(api::command))
        .fallback_service(ServeDir::new(&dist).fallback(ServeFile::new(index)))
        .with_state(events_tx);

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("failed to bind {addr} — {e}");
            std::process::exit(1);
        }
    };

    println!("megathrone-web listening on http://{addr}/");
    println!("data directory: {}", data_dir.display());
    if !dist_ready {
        println!(
            "warning: web UI not found at {} — serving a placeholder",
            dist.display()
        );
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("server error");

    // the listener is gone — unwind a running session exactly like the
    // desktop app's RunEvent::Exit does (sidecars, system proxy, marker)
    connection::shutdown();
}

/// Ctrl+C / SIGTERM: drain the HTTP listener first, then the session.
async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let terminate = match signal(SignalKind::terminate()) {
            Ok(terminate) => terminate,
            Err(_) => {
                ctrl_c.await.ok();
                return;
            }
        };
        let mut terminate = terminate;
        tokio::select! {
            _ = ctrl_c => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
    }
}
