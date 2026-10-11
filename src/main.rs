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

    let lifetime = cli.command.lifetime();
    let thumbs = if lifetime.no_thumbnails {
        None
    } else {
        Some(transfer::thumbnail::Thumbnailer::new(
            transfer::thumbnail::HeicTool::detect(),
            transfer::thumbnail::VideoTool::detect(),
        ))
    };

    print_banner(
        &share,
        &url,
        &net,
        &code,
        upload.as_ref(),
        thumbs.as_ref(),
        lifetime,
    );

    let state = server::state(auth, share, upload, thumbs, lifetime.once);
    server::run(SocketAddr::new(net.bind, net.port), state, lifetime.timeout).await
}

/// Everything a person needs at a glance, in the order they need it: what
/// is shared, where to point the other device, the code, the QR. The
/// quieter details (uploads, thumbnails, stop conditions) come after.
fn print_banner(
    share: &files::Share,
    url: &str,
    net: &cli::NetOptions,
    code: &str,
    upload: Option<&transfer::upload::UploadConfig>,
    thumbs: Option<&transfer::thumbnail::Thumbnailer>,
    lifetime: &cli::LifetimeOptions,
) {
    use utils::term::{bold, cyan, dim, green, human_size, yellow};

    let listing = share.list();
    let total: u64 = listing.iter().map(|f| f.size).sum();
    let what = match listing.len() {
        1 => format!("{} ({})", bold(&share.display_name()), human_size(total)),
        n => format!(
            "{} ({n} files, {})",
            bold(&share.display_name()),
            human_size(total)
        ),
    };
    println!();
    println!(
        "  {}  {}  sharing {what}",
        bold("LANShare"),
        dim(env!("CARGO_PKG_VERSION"))
    );
    for root in share.roots() {
        println!("  {}", dim(&root.path.display().to_string()));
    }
    println!();
    println!("  {}   {}", dim("URL "), cyan(&bold(url)));
    println!("  {}   {}", dim("Code"), green(&bold(code)));
    println!();
    // The QR carries only the URL. Putting the code in it would turn "can see
    // the screen" into "can read the files", which is a different promise.
    if let Some(qr) = utils::qr::render(url) {
        for line in qr.lines() {
            println!("  {line}");
        }
    }
    println!(
        "  {}",
        dim(&format!("On this computer: http://127.0.0.1:{}", net.port))
    );
    if let Some(upload) = upload {
        println!("  {} {}", yellow("Uploads enabled →"), upload.dir.display());
    }
    match thumbs {
        None => println!("  {}", dim("Thumbnails off")),
        Some(t) => {
            let heic = match t.heic_tool() {
                Some(tool) => format!("HEIC via {}", tool.describe()),
                None => "no HEIC decoder (install ImageMagick or libheif-examples)".to_string(),
            };
            let video = match t.video_tool() {
                Some(tool) => format!("video via {}", tool.describe()),
                None => "no video decoder (install ffmpeg)".to_string(),
            };
            println!("  {}", dim(&format!("Thumbnails: {heic} · {video}")));
        }
    }
    if lifetime.once {
        println!("  {}", dim("Stops after the first completed transfer"));
    }
    if let Some(t) = lifetime.timeout {
        println!(
            "  {}",
            dim(&format!("Stops after {}", humantime::format_duration(t)))
        );
    }
    println!("  {}", dim("Press Ctrl+C to stop"));
    println!();
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
