#![forbid(unsafe_code)]
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
        #[arg(value_parser)]
        path: PathBuf,

        /// set [rustup] sync = false
        #[arg(long)]
        ignore_rustup: bool,
    },

    /// Update an existing mirror directory.
    Sync {
        /// Mirror directory.
        #[arg(value_parser)]
        path: PathBuf,

        /// cargo-vendor directory.
        #[arg(long)]
        vendor_path: Option<PathBuf>,

        /// cargo-lock file.
        #[arg(long = "cargo-lock")]
        cargo_lock_filepath: Option<PathBuf>,

        #[arg(long)]
        skip_rustup: bool,
    },

    /// Rewrite the config.json within crates.io-index.
    ///
    /// This can be used if rewriting config.json is
    /// required to be an extra step after syncing.
    #[command(name = "rewrite")]
    Rewrite {
        /// Mirror directory.
        #[arg(value_parser)]
        path: PathBuf,

        /// Base URL used for rewriting. Overrides value in mirror.toml.
        #[arg(short, long)]
        base_url: Option<String>,
    },

    /// Serve a mirror directory.
    #[command(name = "serve")]
    Serve {
        /// Mirror directory.
        #[arg(value_parser)]
        path: PathBuf,

        /// IP address to listen on. Defaults to listening on everything.
        #[arg(short, long)]
        listen: Option<IpAddr>,

        /// Port to listen on.
        /// Defaults to 8080, or 8443 if TLS certificate provided.
        #[arg(short, long)]
        port: Option<u16>,

        /// Path to a TLS certificate file. This enables TLS.
        /// Also requires key_path.
        #[arg(long)]
        cert_path: Option<PathBuf>,

        /// Path to a TLS key file.
        /// Also requires cert_path.
        #[arg(long)]
        key_path: Option<PathBuf>,
    },

    /// List platforms currently available.
    ///
    /// This is useful for finding what can be used for
    /// limiting platforms in mirror.toml.
    #[command(name = "list-platforms")]
    ListPlatforms {
        #[arg(long, default_value = "https://static.rust-lang.org")]
        source: String,

        #[arg(long, default_value = "nightly")]
        channel: String,
    },

    /// Verify coherence between local mirror and local crates.io-index.
    /// If any missing crate is found, ask to user before downloading by default.
    #[command(name = "verify", alias = "check")]
    Verify {
        /// Mirror directory.
        #[arg(value_parser)]
        path: PathBuf,

        /// Dry run, i.e. no change will be made to the mirror.
        /// Missing crates are just printed to stdout, not downloaded.
        #[arg(long)]
        dry_run: bool,

        /// Assume yes from user.
        /// Ignored if dry-run is supplied.
        #[arg(long)]
        assume_yes: bool,

        /// cargo-vendor directory.
        #[arg(value_parser)]
        vendor_path: Option<PathBuf>,

        /// cargo-lock file.
        #[arg(long = "cargo-lock")]
        cargo_lock_filepath: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() {
    env_logger::init();
    let opt = Panamax::parse();
    match opt {
        Panamax::Init {
            path,
            ignore_rustup,
        } => mirror::init(&path, ignore_rustup),
        Panamax::Sync {
            path,
            vendor_path,
            cargo_lock_filepath,
            skip_rustup,
        } => mirror::sync(&path, vendor_path, cargo_lock_filepath, skip_rustup).await,
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
            vendor_path,
            cargo_lock_filepath,
        } => mirror::verify(path, dry_run, assume_yes, vendor_path, cargo_lock_filepath).await,
    }
    .unwrap_or_else(|e| eprintln!("Panamax command failed! {e}"));
}
