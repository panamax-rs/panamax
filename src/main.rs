use clap::Parser;
use std::{net::IpAddr, path::PathBuf};

mod crates;
mod crates_index;
mod download;
mod mirror;
mod progress_bar;
mod rustup;
mod serve;
mod verify;

/// Mirror rustup and crates.io repositories, for offline Rust and cargo usage.
#[derive(Debug, Parser)]
enum Panamax {
    /// Create a new mirror directory.
    Init {
        #[clap(parse(from_os_str))]
        path: PathBuf,
    },

    /// Update an existing mirror directory.
    Sync {
        /// Mirror directory.
        #[clap(parse(from_os_str))]
        path: PathBuf,
        /// cargo-vendor directory.
        #[clap(parse(from_os_str))]
        vendor_path: Option<PathBuf>,
    },

    /// Rewrite the config.json within crates.io-index.
    ///
    /// This can be used if rewriting config.json is
    /// required to be an extra step after syncing.
    #[clap(name = "rewrite")]
    Rewrite {
        /// Mirror directory.
        #[clap(parse(from_os_str))]
        path: PathBuf,

        /// Base URL used for rewriting. Overrides value in mirror.toml.
        #[clap(short, long)]
        base_url: Option<String>,
    },

    /// Serve a mirror directory.
    #[clap(name = "serve")]
    Serve {
        /// Mirror directory.
        #[clap(parse(from_os_str))]
        path: PathBuf,

        /// IP address to listen on. Defaults to listening on everything.
        #[clap(short, long)]
        listen: Option<IpAddr>,

        /// Port to listen on.
        /// Defaults to 8080, or 8443 if TLS certificate provided.
        #[clap(short, long)]
        port: Option<u16>,

        /// Path to a TLS certificate file. This enables TLS.
        /// Also requires key_path.
        #[clap(long)]
        cert_path: Option<PathBuf>,

        /// Path to a TLS key file.
        /// Also requires cert_path.
        #[clap(long)]
        key_path: Option<PathBuf>,
    },

    /// List platforms currently available.
    ///
    /// This is useful for finding what can be used for
    /// limiting platforms in mirror.toml.
    #[clap(name = "list-platforms")]
    ListPlatforms {
        #[clap(long, default_value = "https://static.rust-lang.org")]
        source: String,
        #[clap(long, default_value = "nightly")]
        channel: String,
    },

    /// Verify coherence between local mirror and local crates.io-index.
    /// If any missing crate is found, ask to user before downloading by default.
    #[clap(name = "verify", alias = "check")]
    Verify {
        /// Mirror directory.
        #[clap(parse(from_os_str))]
        path: PathBuf,

        /// Dry run, i.e. no change will be made to the mirror.
        /// Missing crates are just printed to stdout, not downloaded.
        #[clap(long)]
        dry_run: bool,

        /// Assume yes from user.
        /// Ignored if dry-run is supplied.
        #[clap(long)]
        assume_yes: bool,
    },
}

#[tokio::main]
async fn main() {
    env_logger::init();
    let opt = Panamax::parse();
    match opt {
        Panamax::Init { path } => mirror::init(&path),
        Panamax::Sync { path, vendor_path } => mirror::sync(&path, vendor_path).await,
        Panamax::Rewrite { path, base_url } => mirror::rewrite(&path, base_url),
        Panamax::Serve {
            path,
            listen,
            port,
            cert_path,
            key_path,
        } => mirror::serve(path, listen, port, cert_path, key_path).await,
        Panamax::ListPlatforms { source, channel } => mirror::list_platforms(source, channel).await,
        Panamax::Verify {
            path,
            dry_run,
            assume_yes,
        } => mirror::verify(path, dry_run, assume_yes).await,
    }
    .unwrap_or_else(|e| eprintln!("Panamax command failed! {}", e));
}
