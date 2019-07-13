use std::path::PathBuf;
use structopt::StructOpt;

/// Mirror rustup and crates.io repositories, for offline Rust and cargo usage.
#[derive(Debug, StructOpt)]
#[structopt(name = "panamax")]
struct Opt {
    /// Output directory
    #[structopt(parse(from_os_str))]
    out: PathBuf,

    /// Only mirror the rustup files
    #[structopt(short, long = "rustup")]
    rustup_only: bool,

    /// Only mirror crates.io
    #[structopt(short, long = "crates")]
    crates_only: bool,

    /// Number of threads to download with
    #[structopt(short, long = "threads")]
    threads: Option<usize>,

    /// Rewrite the download URL in crates.io-index (should point to the /crates directory)
    #[structopt(short = "u", long = "url")]
    rewrite_url: Option<String>,
}

fn main() {
    let opt = Opt::from_args();
    dbg!(opt);
}
