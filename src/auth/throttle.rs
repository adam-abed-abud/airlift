use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// Wrong guesses a single client may make before being locked out.
const FAILURES_BEFORE_LOCK: u32 = 5;
/// First lockout length; doubles on every further lockout for that client.
const BASE_LOCKOUT: Duration = Duration::from_secs(30);
const MAX_LOCKOUT: Duration = Duration::from_secs(15 * 60);
/// Wrong guesses across *all* clients before the PIN is replaced. With
/// lockouts this is on the order of a dozen distinct IPs misbehaving, which
/// on a home LAN means something is wrong and a fresh code is the safe move.
const GLOBAL_FAILURES_BEFORE_ROTATE: u32 = 30;
/// Keep the per-IP table from growing without bound on a hostile network.
const MAX_TRACKED_CLIENTS: usize = 4096;

#[derive(Debug, Default)]
struct Client {
    failures: u32,
    lockouts: u32,
    locked_until: Option<Instant>,
    last_seen: Option<Instant>,
}

/// Per-client and global brute-force protection for PIN attempts.
#[derive(Debug, Default)]
pub struct Throttle {
    clients: HashMap<IpAddr, Client>,
    global_failures: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FailureOutcome {
    /// Failure recorded, client may try again immediately.
    Counted,
    /// Client just crossed the threshold and is locked for the duration.
    Locked(Duration),
    /// Global threshold crossed; caller must replace the PIN.
    RotatePin,
}

impl Throttle {
    /// If the client is currently locked out, how long it must wait.
    pub fn retry_after(&mut self, ip: IpAddr) -> Option<Duration> {
        self.retry_after_at(ip, Instant::now())
    }

    pub fn record_failure(&mut self, ip: IpAddr) -> FailureOutcome {
        self.record_failure_at(ip, Instant::now())
    }

    pub fn record_success(&mut self, ip: IpAddr) {
        self.clients.remove(&ip);
    }

    fn retry_after_at(&mut self, ip: IpAddr, now: Instant) -> Option<Duration> {
        let client = self.clients.get_mut(&ip)?;
        match client.locked_until {
            Some(until) if until > now => Some(until - now),
            Some(_) => {
                // Lockout expired: the client gets a fresh (smaller) budget
                // but the lockout count stays, so the next lock is longer.
                client.locked_until = None;
                client.failures = 0;
                None
            }
            None => None,
        }
    }

    fn record_failure_at(&mut self, ip: IpAddr, now: Instant) -> FailureOutcome {
        self.global_failures += 1;
        if self.global_failures >= GLOBAL_FAILURES_BEFORE_ROTATE {
            // A new PIN makes all accumulated knowledge worthless, so the
            // counters can start over too.
            self.global_failures = 0;
            self.clients.clear();
            return FailureOutcome::RotatePin;
        }

        if self.clients.len() >= MAX_TRACKED_CLIENTS && !self.clients.contains_key(&ip) {
            self.evict_oldest();
        }

        let client = self.clients.entry(ip).or_default();
        client.failures += 1;
        client.last_seen = Some(now);
        if client.failures >= FAILURES_BEFORE_LOCK {
            let lockout = BASE_LOCKOUT
                .checked_mul(1 << client.lockouts.min(16))
                .unwrap_or(MAX_LOCKOUT)
                .min(MAX_LOCKOUT);
            client.lockouts += 1;
            client.failures = 0;
            client.locked_until = Some(now + lockout);
            return FailureOutcome::Locked(lockout);
        }
        FailureOutcome::Counted
    }

    fn evict_oldest(&mut self) {
        if let Some(oldest) = self
            .clients
            .iter()
            .min_by_key(|(_, c)| c.last_seen)
            .map(|(ip, _)| *ip)
        {
            self.clients.remove(&oldest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(n: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, n])
    }

    #[test]
    fn locks_after_threshold_and_backs_off() {
        let mut t = Throttle::default();
        let now = Instant::now();
        for _ in 0..FAILURES_BEFORE_LOCK - 1 {
            assert_eq!(t.record_failure_at(ip(1), now), FailureOutcome::Counted);
        }
        assert_eq!(
            t.record_failure_at(ip(1), now),
            FailureOutcome::Locked(BASE_LOCKOUT)
        );
        assert!(t.retry_after_at(ip(1), now).is_some());
        // Other clients are unaffected.
        assert!(t.retry_after_at(ip(2), now).is_none());

        // After the lockout the client may try again; the next lock is longer.
        let later = now + BASE_LOCKOUT + Duration::from_secs(1);
        assert!(t.retry_after_at(ip(1), later).is_none());
        for _ in 0..FAILURES_BEFORE_LOCK - 1 {
            t.record_failure_at(ip(1), later);
        }
        assert_eq!(
            t.record_failure_at(ip(1), later),
            FailureOutcome::Locked(BASE_LOCKOUT * 2)
        );
    }

    #[test]
    fn success_clears_client() {
        let mut t = Throttle::default();
        let now = Instant::now();
        for _ in 0..FAILURES_BEFORE_LOCK {
            t.record_failure_at(ip(1), now);
        }
        t.record_success(ip(1));
        assert!(t.retry_after_at(ip(1), now).is_none());
    }

    #[test]
    fn rotates_pin_after_global_threshold() {
        let mut t = Throttle::default();
        let now = Instant::now();
        let mut rotated = false;
        for n in 0..GLOBAL_FAILURES_BEFORE_ROTATE {
            // Spread across clients so no single one is locked out first.
            if t.record_failure_at(ip(n as u8), now) == FailureOutcome::RotatePin {
                rotated = true;
            }
        }
        assert!(rotated);
        assert_eq!(t.global_failures, 0);
    }
}
