use tracing_subscriber::EnvFilter;

/// Initialise `tracing` on stderr.
///
/// The activity feed on stdout is what people look at; `tracing` is for
/// diagnostics. Without `-v` only warnings and errors get through, so the
/// terminal stays readable. The verbosity flag wins over `RUST_LOG` so a
/// casual `-v` does what people expect, while `RUST_LOG` still works for
/// fine-grained filtering when no flag is given.
pub fn init(verbosity: u8) {
    let default = match verbosity {
        0 => "lanshare=warn,tower_http=warn",
        1 => "lanshare=debug,tower_http=debug",
        _ => "trace",
    };
    let filter = if verbosity == 0 {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default))
    } else {
        EnvFilter::new(default)
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(verbosity > 0)
        .compact()
        .init();
}
