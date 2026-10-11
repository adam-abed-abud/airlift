//! Authentication: a short-lived access code (PIN) exchanged for a session
//! cookie, with throttling to make guessing a six-digit code impractical.
//!
//! The PIN is *authorization*, not encryption: it decides who may use the
//! share, nothing more. See `docs/security.md` for the threat model.

pub mod pin;
pub mod session;
pub mod throttle;

use std::net::IpAddr;
use std::sync::{Mutex, RwLock};
use std::time::Duration;

use tracing::warn;

use crate::utils::{activity, term};

use pin::Pin;
use session::SessionStore;
use throttle::{FailureOutcome, Throttle};

/// Name of the session cookie. Prefixed so it cannot be confused with
/// anything else a browser might hold for this IP:port.
pub const COOKIE_NAME: &str = "airlift_session";

/// Everything the server needs to decide whether a request is allowed.
pub struct Auth {
    pin: RwLock<Pin>,
    throttle: Mutex<Throttle>,
    pub sessions: SessionStore,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LoginError {
    /// Too many failed attempts from this client; retry after the duration.
    Throttled(Duration),
    /// Wrong code. The client is told nothing more than that.
    WrongPin,
}

impl Auth {
    pub fn new(pin: Pin) -> Self {
        Self {
            pin: RwLock::new(pin),
            throttle: Mutex::new(Throttle::default()),
            sessions: SessionStore::default(),
        }
    }

    /// The current code, formatted for humans (`482-913`).
    pub fn pin_display(&self) -> String {
        self.pin.read().unwrap().display()
    }

    /// Exchange a candidate code for a session token.
    ///
    /// Throttling is checked *before* the PIN so a locked-out client learns
    /// nothing about whether its guess was right. After enough failures from
    /// anywhere the PIN is rotated: a brute-forcer then starts from scratch,
    /// and the person at the terminal sees a loud warning plus the new code.
    pub fn login(&self, ip: IpAddr, candidate: &str, device: String) -> Result<String, LoginError> {
        let mut throttle = self.throttle.lock().unwrap();
        if let Some(wait) = throttle.retry_after(ip) {
            return Err(LoginError::Throttled(wait));
        }

        if self.pin.read().unwrap().verify(candidate) {
            throttle.record_success(ip);
            return Ok(self.sessions.create(device));
        }

        match throttle.record_failure(ip) {
            FailureOutcome::RotatePin => {
                let fresh = Pin::generate();
                let shown = fresh.display();
                *self.pin.write().unwrap() = fresh;
                warn!("too many failed login attempts; access code rotated");
                activity::problem(format!(
                    "Too many wrong codes. New access code: {}",
                    term::bold(&shown)
                ));
                Err(LoginError::WrongPin)
            }
            FailureOutcome::Locked(wait) => Err(LoginError::Throttled(wait)),
            FailureOutcome::Counted => Err(LoginError::WrongPin),
        }
    }
}
