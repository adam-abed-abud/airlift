//! Command-line interface.
//!
//! The CLI is intentionally tiny: two verbs (`send` a file, `serve` directories)
//! plus a handful of network options. Everything else is decided at runtime so
//! that the "share something now" flow stays a single command.

use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "lanshare", version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Increase log verbosity (-v for debug, -vv for trace).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Share a single file for download.
    Send {
        /// File to share.
        file: PathBuf,

        #[command(flatten)]
        net: NetOptions,

        #[command(flatten)]
        upload: UploadOptions,

        #[command(flatten)]
        lifetime: LifetimeOptions,
    },

    /// Share one or more directories for browsing and download.
    Serve {
        /// Directories to share. Each one becomes a share root.
        #[arg(required = true)]
        dirs: Vec<PathBuf>,

        /// Also list dot-files and dot-directories. Off by default because
        /// `.git`, `.env` and friends are rarely what you meant to share.
        #[arg(long)]
        include_hidden: bool,

        #[command(flatten)]
        net: NetOptions,

        #[command(flatten)]
        upload: UploadOptions,

        #[command(flatten)]
        lifetime: LifetimeOptions,
    },
}

/// When to stop without being told. Both exist so a share cannot be left
/// running by accident after the one transfer it was started for.
#[derive(Args, Debug, Clone, Default)]
pub struct LifetimeOptions {
    /// Exit after the first completed transfer (a full download, or an
    /// upload request that saved at least one file).
    #[arg(long)]
    pub once: bool,

    /// Exit after this long, e.g. `30m`, `2h`, `1h30m`.
    #[arg(long, value_name = "DURATION", value_parser = humantime::parse_duration)]
    pub timeout: Option<Duration>,
}

/// Options that control where the server listens and which URL is advertised.
#[derive(Args, Debug, Clone)]
pub struct NetOptions {
    /// TCP port to listen on.
    #[arg(short, long, default_value_t = 8080)]
    pub port: u16,

    /// Interface address to bind. The default exposes the share on every
    /// interface, which is what you want on a home LAN; narrow it with e.g.
    /// `--bind 192.168.1.105` to limit exposure on multi-homed machines.
    #[arg(long, default_value = "0.0.0.0")]
    pub bind: IpAddr,

    /// Hostname or IP to print in the URL and QR code. By default the LAN
    /// address of the default route is detected automatically.
    #[arg(long)]
    pub host: Option<String>,
}

/// Uploads are off unless asked for: a share is read-only by default so
/// that handing out the code never silently grants write access.
#[derive(Args, Debug, Clone)]
pub struct UploadOptions {
    /// Let connected devices upload files to this computer.
    #[arg(long)]
    pub allow_upload: bool,

    /// Where uploads are written. Defaults to the first shared directory
    /// for `serve`, and to the current directory for `send`.
    #[arg(long, value_name = "DIR", requires = "allow_upload")]
    pub upload_dir: Option<PathBuf>,

    /// Maximum size of one upload request, e.g. `500M` or `10G`.
    #[arg(long, value_name = "SIZE", default_value = "10G", value_parser = parse_size)]
    pub max_upload: u64,
}

impl Command {
    pub fn net(&self) -> &NetOptions {
        match self {
            Command::Send { net, .. } | Command::Serve { net, .. } => net,
        }
    }

    pub fn upload(&self) -> &UploadOptions {
        match self {
            Command::Send { upload, .. } | Command::Serve { upload, .. } => upload,
        }
    }

    pub fn lifetime(&self) -> &LifetimeOptions {
        match self {
            Command::Send { lifetime, .. } | Command::Serve { lifetime, .. } => lifetime,
        }
    }
}

/// Parse `123`, `500M`, `10G`, `1.5G` (binary units, as disks are shown).
pub fn parse_size(input: &str) -> Result<u64, String> {
    let s = input.trim();
    let split = s
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(s.len());
    let (number, unit) = s.split_at(split);
    let value: f64 = number
        .parse()
        .map_err(|_| format!("invalid size `{input}`"))?;
    let multiplier: f64 = match unit.trim().to_ascii_uppercase().as_str() {
        "" | "B" => 1.0,
        "K" | "KB" | "KIB" => 1024.0,
        "M" | "MB" | "MIB" => 1024.0 * 1024.0,
        "G" | "GB" | "GIB" => 1024.0 * 1024.0 * 1024.0,
        "T" | "TB" | "TIB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        other => return Err(format!("unknown size unit `{other}`")),
    };
    let bytes = value * multiplier;
    if bytes < 1.0 || !bytes.is_finite() {
        return Err(format!("size `{input}` must be at least 1 byte"));
    }
    Ok(bytes as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_size("123").unwrap(), 123);
        assert_eq!(parse_size("1K").unwrap(), 1024);
        assert_eq!(parse_size("500M").unwrap(), 500 * 1024 * 1024);
        assert_eq!(parse_size("1.5g").unwrap(), 3 * 512 * 1024 * 1024);
        assert!(parse_size("0").is_err());
        assert!(parse_size("abc").is_err());
        assert!(parse_size("10X").is_err());
    }
}
