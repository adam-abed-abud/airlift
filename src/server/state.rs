use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::auth::Auth;
use crate::files::Share;
use crate::transfer::upload::UploadConfig;

/// Scheme the server is reachable on. Only HTTP exists today, but cookie
/// flags and origin checks already branch on this so an HTTPS mode can be
/// added without touching the auth code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Http,
    Https,
}

impl Scheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Scheme::Http => "http",
            Scheme::Https => "https",
        }
    }
}

/// Shared, immutable-after-start state handed to every handler.
#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<Auth>,
    pub share: Arc<Share>,
    /// `None` means uploads are disabled and the route is not even mounted.
    pub upload: Option<Arc<UploadConfig>>,
    pub scheme: Scheme,
    /// Cancelling this stops the server. Handlers use it for `--once`.
    pub shutdown: CancellationToken,
    /// Stop after the first completed transfer.
    pub once: bool,
}

impl AppState {
    /// Called by transfer handlers when a transfer has fully completed.
    /// Only acts in `--once` mode; harmless otherwise.
    pub fn transfer_completed(&self, what: &str) {
        if self.once && !self.shutdown.is_cancelled() {
            tracing::info!("{what} complete; stopping (--once)");
            self.shutdown.cancel();
        }
    }
}
