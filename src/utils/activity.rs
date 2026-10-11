//! The live activity feed: one plain line per thing a person would want to
//! know about while the share is running ("iPad connected", "movie.mp4
//! sent"). This is the tool's primary output, so it goes to stdout without
//! log levels or targets; `tracing` diagnostics go to stderr and are quiet
//! unless `-v` is given.

use super::term;

#[derive(Debug, Clone, Copy)]
pub enum Kind {
    /// Something started or a device appeared.
    Info,
    /// A transfer finished.
    Done,
    /// Something was refused or went wrong.
    Problem,
}

/// Print a timestamped activity line.
pub fn event(kind: Kind, message: impl AsRef<str>) {
    let time = chrono::Local::now().format("%H:%M:%S").to_string();
    let marker = match kind {
        Kind::Info => term::cyan("▸"),
        Kind::Done => term::green("✓"),
        Kind::Problem => term::red("✗"),
    };
    println!("{}  {marker} {}", term::dim(&time), message.as_ref());
}

pub fn info(message: impl AsRef<str>) {
    event(Kind::Info, message);
}

pub fn done(message: impl AsRef<str>) {
    event(Kind::Done, message);
}

pub fn problem(message: impl AsRef<str>) {
    event(Kind::Problem, message);
}
