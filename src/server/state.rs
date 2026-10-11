use std::sync::Arc;

use crate::auth::Auth;
use crate::files::Share;

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
    pub scheme: Scheme,
}
