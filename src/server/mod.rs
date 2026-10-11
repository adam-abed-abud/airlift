//! HTTP server assembly: router, shared state and the run loop.

use std::net::SocketAddr;

use anyhow::Context;
use axum::{Json, Router, routing::get};
use tokio::net::TcpListener;
use tracing::info;

/// Build the application router. Kept separate from `run` so tests can drive
/// the router without opening a socket.
pub fn router() -> Router {
    Router::new().route("/api/status", get(status))
}

async fn status() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "version": env!("CARGO_PKG_VERSION") }))
}

/// Serve until Ctrl+C. Returns once the listener has shut down.
pub async fn run(addr: SocketAddr) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;
    info!("listening on {}", listener.local_addr()?);

    axum::serve(listener, router())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;
    Ok(())
}

async fn shutdown_signal() {
    // Ignoring the error is fine: if the signal handler can't be installed the
    // user can still kill the process, and we'd rather serve than bail.
    let _ = tokio::signal::ctrl_c().await;
    info!("shutting down");
}
