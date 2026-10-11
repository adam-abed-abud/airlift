use subtle::ConstantTimeEq;

/// Number of digits in the access code. Six is a deliberate trade-off: short
/// enough to type on a phone from across the room, and with throttling (see
/// `throttle.rs`) still far too many combinations to guess in a session.
pub const PIN_DIGITS: usize = 6;
const PIN_SPACE: u32 = 1_000_000;

/// A freshly generated access code. Never configured, never persisted.
#[derive(Clone)]
pub struct Pin(String);

impl Pin {
    /// Generate a uniformly random code from the OS CSPRNG.
    pub fn generate() -> Self {
        // Rejection sampling: `u32 % 1_000_000` alone would slightly favour
        // low codes because 2^32 is not a multiple of 10^6. The bias is tiny,
        // but correct is cheap here, so do it properly.
        let limit = u32::MAX - (u32::MAX % PIN_SPACE);
        loop {
            let n = getrandom::u32().expect("OS random source unavailable");
            if n < limit {
                return Pin(format!("{:06}", n % PIN_SPACE));
            }
        }
    }

    /// Construct from a known value. Exists for tests; the binary never
    /// accepts a configured code on purpose.
    pub fn from_digits(s: &str) -> Option<Self> {
        (s.len() == PIN_DIGITS && s.bytes().all(|b| b.is_ascii_digit())).then(|| Pin(s.to_string()))
    }

    /// Human-friendly form, e.g. `482-913`. The dash is only a reading aid;
    /// `verify` ignores it.
    pub fn display(&self) -> String {
        format!("{}-{}", &self.0[..3], &self.0[3..])
    }

    /// Compare against user input in constant time. Separators and spaces
    /// are stripped first so people can type the code exactly as shown.
    pub fn verify(&self, candidate: &str) -> bool {
        let digits: String = candidate.chars().filter(|c| c.is_ascii_digit()).collect();
        // Length mismatch is already public information (the UI says six
        // digits), so an early return here leaks nothing useful.
        if digits.len() != PIN_DIGITS {
            return false;
        }
        digits.as_bytes().ct_eq(self.0.as_bytes()).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_pin_is_six_digits() {
        let pin = Pin::generate();
        assert_eq!(pin.0.len(), PIN_DIGITS);
        assert!(pin.0.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn verify_ignores_formatting() {
        let pin = Pin::from_digits("482913").unwrap();
        assert!(pin.verify("482913"));
        assert!(pin.verify("482-913"));
        assert!(pin.verify(" 482 913 "));
        assert!(!pin.verify("482914"));
        assert!(!pin.verify("48291"));
        assert!(!pin.verify(""));
    }

    #[test]
    fn display_inserts_dash() {
        assert_eq!(Pin::from_digits("000123").unwrap().display(), "000-123");
    }
}
