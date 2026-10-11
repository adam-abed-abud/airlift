//! `main` only parses arguments, sets up logging and hands over to the server.
//! All interesting behaviour lives in the library modules.

use std::net::SocketAddr;

use clap::Parser;
use lanshare::{auth, cli, files, server, transfer, utils};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    utils::log::init(cli.verbose);

    let net = cli.command.net().clone();
    let share = match &cli.command {
        cli::Command::Send { file, .. } => files::Share::single_file(file)?,
        cli::Command::Serve {
            dirs,
            include_hidden,
            ..
        } => files::Share::directories(dirs, *include_hidden)?,
    };
    let upload = resolve_upload(&cli.command, &share)?;
    let host = utils::net::advertised_host(net.host.as_deref(), net.bind);
    let url = format!("http://{host}:{}", net.port);

    // A fresh code every run: nothing to configure, nothing to leak later.
    let auth = auth::Auth::new(auth::pin::Pin::generate());
    let code = auth.pin_display();

    println!("LANShare");
    for root in share.roots() {
        println!("Sharing: {}", root.path.display());
    }
    if let Some(upload) = &upload {
        println!("Uploads: enabled -> {}", upload.dir.display());
    }
    println!("URL: {url}");
    println!("Access code: {code}");
    // The QR carries only the URL. Putting the code in it would turn "can see
    // the screen" into "can read the files", which is a different promise.
    if let Some(qr) = utils::qr::render(&url) {
        println!("{qr}");
    }
    let lifetime = cli.command.lifetime();
    let thumbs = if lifetime.no_thumbnails {
        None
    } else {
        let tool = transfer::thumbnail::HeicTool::detect();
        match &tool {
            Some(t) => println!(
                "Thumbnails: built-in for JPEG/PNG/GIF/WebP, HEIC via {}",
                t.describe()
            ),
            None => println!(
                "Thumbnails: built-in for JPEG/PNG/GIF/WebP; no HEIC decoder found \
                 (install ImageMagick with HEIC support, or libheif-examples)"
            ),
        }
        Some(transfer::thumbnail::Thumbnailer::new(tool))
    };
    if lifetime.once {
        println!("Stops after the first completed transfer (--once)");
    }
    if let Some(t) = lifetime.timeout {
        println!(
            "Stops automatically after {}",
            humantime::format_duration(t)
        );
    }
    println!("Press Ctrl+C to stop");

    let state = server::state(auth, share, upload, thumbs, lifetime.once);
    server::run(SocketAddr::new(net.bind, net.port), state, lifetime.timeout).await
}

/// Turn the upload flags into a config, picking a sensible default
/// destination when `--upload-dir` is not given.
fn resolve_upload(
    command: &cli::Command,
    share: &files::Share,
) -> anyhow::Result<Option<transfer::upload::UploadConfig>> {
    let opts = command.upload();
    if !opts.allow_upload {
        return Ok(None);
    }
    let dir = match (&opts.upload_dir, command) {
        (Some(dir), _) => dir.clone(),
        (None, cli::Command::Serve { .. }) => share.roots()[0].path.clone(),
        (None, cli::Command::Send { .. }) => std::env::current_dir()?,
    };
    Ok(Some(transfer::upload::UploadConfig::new(
        &dir,
        opts.max_upload,
    )?))
}
