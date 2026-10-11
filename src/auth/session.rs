use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A session ends after this much inactivity.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);
/// ...and unconditionally after this long, even if it is in constant use.
pub const MAX_LIFETIME: Duration = Duration::from_secs(12 * 60 * 60);
/// 32 random bytes = 256 bits: unguessable, and far beyond what the PIN gives.
const TOKEN_BYTES: usize = 32;

#[derive(Debug)]
struct Session {
    created: Instant,
    last_seen: Instant,
    /// Friendly label like `iPad (192.168.1.23)`, for the activity feed.
    device: String,
}

/// In-memory session table. Deliberately not persisted: stopping the program
/// must revoke every session, and the simplest way to guarantee that is to
/// keep them only in RAM.
#[derive(Debug, Default)]
pub struct SessionStore {
    sessions: Mutex<HashMap<String, Session>>,
}

impl SessionStore {
    pub fn create(&self, device: String) -> String {
        let token = random_token();
        let now = Instant::now();
        let mut sessions = self.sessions.lock().unwrap();
        prune(&mut sessions, now);
        sessions.insert(
            token.clone(),
            Session {
                created: now,
                last_seen: now,
                device,
            },
        );
        token
    }

    /// Device label of a live session, if any.
    pub fn device(&self, token: &str) -> Option<String> {
        self.sessions
            .lock()
            .unwrap()
            .get(token)
            .map(|s| s.device.clone())
    }

    /// Check a token and, if valid, extend its idle window.
    pub fn validate(&self, token: &str) -> bool {
        let now = Instant::now();
        let mut sessions = self.sessions.lock().unwrap();
        match sessions.get_mut(token) {
            Some(s) if is_live(s, now) => {
                s.last_seen = now;
                true
            }
            Some(_) => {
                sessions.remove(token);
                false
            }
            None => false,
        }
    }

    pub fn revoke(&self, token: &str) {
        self.sessions.lock().unwrap().remove(token);
    }
}

fn is_live(s: &Session, now: Instant) -> bool {
    now.duration_since(s.last_seen) < IDLE_TIMEOUT && now.duration_since(s.created) < MAX_LIFETIME
}

fn prune(sessions: &mut HashMap<String, Session>, now: Instant) {
    sessions.retain(|_, s| is_live(s, now));
}

/// Hex-encoded random token. Hex rather than base64 keeps the cookie value
/// free of characters that need quoting.
fn random_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes).expect("OS random source unavailable");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_validate_revoke() {
        let store = SessionStore::default();
        let token = store.create("test".into());
        assert_eq!(token.len(), TOKEN_BYTES * 2);
        assert!(store.validate(&token));
        assert!(!store.validate("nope"));
        store.revoke(&token);
        assert!(!store.validate(&token));
    }

    #[test]
    fn expired_sessions_are_dropped() {
        let store = SessionStore::default();
        let token = store.create("test".into());
        {
            let mut sessions = store.sessions.lock().unwrap();
            let s = sessions.get_mut(&token).unwrap();
            s.last_seen = Instant::now() - IDLE_TIMEOUT - Duration::from_secs(1);
        }
        assert!(!store.validate(&token));
        assert!(store.sessions.lock().unwrap().is_empty());
    }
}
