//! Command-line interface.
//!
//! The CLI is intentionally tiny: two verbs (`send` a file, `serve` directories)
//! plus a handful of network options. Everything else is decided at runtime so
//! that the "share something now" flow stays a single command.

use std::net::IpAddr;
use std::path::PathBuf;

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
    },
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

impl Command {
    pub fn net(&self) -> &NetOptions {
        match self {
            Command::Send { net, .. } | Command::Serve { net, .. } => net,
        }
    }
}
