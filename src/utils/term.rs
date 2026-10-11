//! ANSI colour helpers for the terminal banner and activity lines.
//!
//! Colour is only used when stdout is a terminal, `NO_COLOR` is unset and
//! `TERM` is not `dumb`, so piping the output into a file or another
//! program yields plain text. No crate: five escape codes do not need one.

use std::io::IsTerminal;
use std::sync::OnceLock;

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::io::stdout().is_terminal()
            && std::env::var_os("NO_COLOR").is_none()
            && std::env::var("TERM").map(|t| t != "dumb").unwrap_or(true)
    })
}

fn wrap(code: &str, text: &str) -> String {
    if enabled() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

pub fn bold(text: &str) -> String {
    wrap("1", text)
}

pub fn dim(text: &str) -> String {
    wrap("2", text)
}

pub fn green(text: &str) -> String {
    wrap("32", text)
}

pub fn cyan(text: &str) -> String {
    wrap("36", text)
}

pub fn yellow(text: &str) -> String {
    wrap("33", text)
}

pub fn red(text: &str) -> String {
    wrap("31", text)
}

/// Human-readable byte count: `1.2 GB`, `340 KB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// `40 s`, `2 min 5 s`, `0.4 s`.
pub fn human_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 1.0 {
        // Floor at 0.1 so a LAN-speed small file does not read as "0.0 s".
        format!("{:.1} s", secs.max(0.1))
    } else if secs < 60.0 {
        format!("{secs:.0} s")
    } else {
        let m = (secs / 60.0).floor() as u64;
        let s = (secs % 60.0).round() as u64;
        format!("{m} min {s} s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(1_300_000_000), "1.2 GB");
        assert_eq!(human_size(52_428_800), "50 MB");
    }

    #[test]
    fn durations() {
        use std::time::Duration;
        assert_eq!(human_duration(Duration::from_millis(400)), "0.4 s");
        assert_eq!(human_duration(Duration::from_millis(3)), "0.1 s");
        assert_eq!(human_duration(Duration::from_secs(40)), "40 s");
        assert_eq!(human_duration(Duration::from_secs(125)), "2 min 5 s");
    }
}
