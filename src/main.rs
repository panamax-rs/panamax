use std::{net::IpAddr, path::PathBuf};
use structopt::StructOpt;

mod crates;
mod crates_index;
mod download;
mod mirror;
mod progress_bar;
mod rustup;
mod serve;
mod verify;

/// Mirror rustup and crates.io repositories, for offline Rust and cargo usage.
#[derive(Debug, StructOpt)]
enum Panamax {
    /// Create a new mirror directory.
    #[structopt(name = "init", alias = "new")]
    Init {
        /// Directory to store the mirror.
        #[structopt(parse(from_os_str))]
        path: PathBuf,
    },

    /// Update an existing mirror directory.
    #[structopt(name = "sync", alias = "run")]
    Sync {
        /// Mirror directory.
        #[structopt(parse(from_os_str))]
        path: PathBuf,
    },

    /// Rewrite the config.json within crates.io-index.
    ///
    /// This can be used if rewriting config.json is
    /// required to be an extra step after syncing.
    #[structopt(name = "rewrite")]
    Rewrite {
        /// Mirror directory.
        #[structopt(parse(from_os_str))]
        path: PathBuf,

        /// Base URL used for rewriting. Overrides value in mirror.toml.
        #[structopt(short, long)]
        base_url: Option<String>,
    },

    /// Serve a mirror directory.
    #[structopt(name = "serve")]
    Serve {
        /// Mirror directory.
        #[structopt(parse(from_os_str))]
        path: PathBuf,

        /// IP address to listen on. Defaults to listening on everything.
        #[structopt(short, long)]
        listen: Option<IpAddr>,

        /// Port to listen on.
        /// Defaults to 8080, or 8443 if TLS certificate provided.
        #[structopt(short, long)]
        port: Option<u16>,

        /// Path to a TLS certificate file. This enables TLS.
        /// Also requires key_path.
        #[structopt(long)]
        cert_path: Option<PathBuf>,

        /// Path to a TLS key file.
        /// Also requires cert_path.
        #[structopt(long)]
        key_path: Option<PathBuf>,
    },

    /// List platforms currently available.
    ///
    /// This is useful for finding what can be used for
    /// limiting platforms in mirror.toml.
    #[structopt(name = "list-platforms")]
    ListPlatforms {
        #[structopt(long, default_value = "https://static.rust-lang.org")]
        source: String,
        #[structopt(long, default_value = "nightly")]
        channel: String,
    },

    /// Verify coherence between local mirror and local crates.io-index.
    #[structopt(name = "verify", alias = "check")]
    Verify {
        /// Mirror directory.
        #[structopt(parse(from_os_str))]
        path: PathBuf,
    },
}

#[tokio::main]
async fn main() {
    env_logger::init();
    let opt = Panamax::from_args();
    match opt {
        Panamax::Init { path } => mirror::init(&path),
        Panamax::Sync { path } => mirror::sync(&path).await,
        Panamax::Rewrite { path, base_url } => mirror::rewrite(&path, base_url),
        Panamax::Serve {
            path,
            listen,
            port,
            cert_path,
            key_path,
        } => mirror::serve(path, listen, port, cert_path, key_path).await,
        Panamax::ListPlatforms { source, channel } => mirror::list_platforms(source, channel).await,
        Panamax::Verify { path } => mirror::verify(path).await,
    }
    .unwrap_or_else(|e| eprintln!("Panamax command failed! {}", e));
}
