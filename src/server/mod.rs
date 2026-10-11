//! HTTP server assembly: router, shared state and the run loop.

pub mod assets;
pub mod error;
pub mod middleware;
pub mod routes;
pub mod state;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderValue, StatusCode, header};
use axum::routing::{get, post};
use axum::{Router, middleware as axum_mw};
use tokio::net::TcpListener;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;
use tracing::{info, warn};

use crate::auth::Auth;
use crate::files::Share;
use crate::transfer::upload::UploadConfig;
pub use state::{AppState, Scheme};

/// How long to wait for in-flight requests after Ctrl+C before giving up.
/// Sessions live only in memory, so an aborted multi-GB download cannot be
/// resumed by the old client anyway; there is no point in waiting for it.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

/// Build the application router. Kept separate from `run` so tests can drive
/// the router without opening a socket.
pub fn router(state: AppState) -> Router {
    // Everything that touches files sits behind the session check. The
    // `route_layer` form means unknown paths still 404 instead of 401, which
    // keeps the API from being a probe for valid routes.
    let mut protected = Router::new()
        .route("/files", get(routes::list_files))
        .route("/files/{id}/download", get(routes::download))
        .route("/files/{id}/preview", get(routes::preview));
    if let Some(upload) = &state.upload {
        // The body limit is per request; a multi-file upload shares it.
        // Mounted only when enabled so a read-only share has no write path
        // at all, not even a disabled one.
        protected = protected.route(
            "/upload",
            post(routes::upload).layer(DefaultBodyLimit::max(upload.max_bytes as usize)),
        );
    }
    let protected = protected.route_layer(axum_mw::from_fn_with_state(
        state.clone(),
        middleware::require_session,
    ));

    let api = Router::new()
        .route("/status", get(routes::status))
        .route("/auth", post(routes::login))
        .route("/logout", post(routes::logout))
        .merge(protected)
        // Without this an unknown /api path falls through to the asset
        // catch-all and answers 405 instead of a plain 404.
        .fallback(|| async { StatusCode::NOT_FOUND })
        .layer(axum_mw::from_fn_with_state(
            state.clone(),
            middleware::same_origin_only,
        ))
        // API responses are per-session and must never be cached or shared.
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ));

    Router::new()
        .route("/", get(assets::index))
        .nest("/api", api)
        // Static assets are the fallback rather than a `/{*path}` route: a
        // catch-all route would also claim unknown /api paths and answer
        // 405 for POSTs, whereas the nested fallback above returns 404.
        .fallback(get(assets::asset))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

pub fn state(auth: Auth, share: Share, upload: Option<UploadConfig>) -> AppState {
    AppState {
        auth: Arc::new(auth),
        share: Arc::new(share),
        upload: upload.map(Arc::new),
        scheme: Scheme::Http,
    }
}

/// Serve until Ctrl+C. Returns once the listener has shut down.
pub async fn run(addr: SocketAddr, state: AppState) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;
    info!("listening on {}", listener.local_addr()?);

    // ConnectInfo gives handlers the peer address for per-client throttling.
    let app = router(state).into_make_service_with_connect_info::<SocketAddr>();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    // `IntoFuture` is only implemented for the serve builder, hence the
    // explicit `async move` wrapper before spawning.
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = stop_rx.await;
            })
            .await
    });

    // Ignoring a failure to install the handler is deliberate: the user can
    // still kill the process, and refusing to serve would be worse.
    let _ = tokio::signal::ctrl_c().await;
    info!("shutting down");
    let _ = stop_tx.send(());

    match tokio::time::timeout(SHUTDOWN_GRACE, server).await {
        Ok(joined) => joined
            .context("server task panicked")?
            .context("server error")?,
        Err(_) => warn!("in-flight transfers did not finish in time; exiting anyway"),
    }
    Ok(())
}
