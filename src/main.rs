//! LANShare: temporary, PIN-protected file sharing over the local network.
//!
//! `main` only parses arguments, sets up logging and hands over to the server.
//! All interesting behaviour lives in the sibling modules.

mod auth;
mod cli;
mod files;
mod server;
mod transfer;
mod utils;

use std::net::SocketAddr;

use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    utils::log::init(cli.verbose);

    let net = cli.command.net().clone();
    let share = match &cli.command {
        cli::Command::Send { file, .. } => files::Share::single_file(file)?,
        cli::Command::Serve { dirs, include_hidden, .. } => {
            files::Share::directories(dirs, *include_hidden)?
        }
    };
    let host = utils::net::advertised_host(net.host.as_deref(), net.bind);
    let url = format!("http://{host}:{}", net.port);

    // A fresh code every run: nothing to configure, nothing to leak later.
    let auth = auth::Auth::new(auth::pin::Pin::generate());
    let code = auth.pin_display();

    println!("LANShare");
    for root in share.roots() {
        println!("Sharing: {}", root.path.display());
    }
    println!("URL: {url}");
    println!("Access code: {code}");
    // The QR carries only the URL. Putting the code in it would turn "can see
    // the screen" into "can read the files", which is a different promise.
    if let Some(qr) = utils::qr::render(&url) {
        println!("{qr}");
    }
    println!("Press Ctrl+C to stop");

    server::run(SocketAddr::new(net.bind, net.port), server::state(auth, share)).await
}
