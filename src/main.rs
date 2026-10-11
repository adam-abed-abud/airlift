//! LANShare: temporary, PIN-protected file sharing over the local network.
//!
//! `main` only parses arguments, sets up logging and hands over to the server.
//! All interesting behaviour lives in the sibling modules.

mod cli;
mod server;
mod utils;

use std::net::SocketAddr;

use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    utils::log::init(cli.verbose);

    let net = cli.command.net().clone();
    let host = utils::net::advertised_host(net.host.as_deref(), net.bind);
    let url = format!("http://{host}:{}", net.port);

    println!("LANShare");
    println!("URL: {url}");
    println!("Press Ctrl+C to stop");

    server::run(SocketAddr::new(net.bind, net.port)).await
}
