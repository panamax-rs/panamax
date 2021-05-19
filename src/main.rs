use std::path::PathBuf;
use structopt::StructOpt;

mod crates;
mod crates_index;
mod download;
mod mirror;
mod progress_bar;
mod rustup;

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
}

fn main() {
    env_logger::init();
    let opt = Panamax::from_args();
    match opt {
        Panamax::Init { path } => mirror::init(&path),
        Panamax::Sync { path } => mirror::sync(&path),
        Panamax::Rewrite { path, base_url } => mirror::rewrite(&path, base_url),
    }
    .unwrap_or_else(|e| eprintln!("Panamax command failed! {}", e));
}
