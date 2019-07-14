// Note: These platforms should match https://github.com/rust-lang/rustup.rs#other-installation-methods

use crate::mirror::{MirrorSection, RustupSection};
use console::style;

static _PLATFORMS: &'static [&'static str] = &[
    "aarch64-linux-android",
    "aarch64-unknown-linux-gnu",
    "arm-linux-androideabi",
    "arm-unknown-linux-gnueabi",
    "arm-unknown-linux-gnueabihf",
    "armv7-linux-androideabi",
    "armv7-unknown-linux-gnueabihf",
    "i686-apple-darwin",
    "i686-linux-android",
    "i686-unknown-linux-gnu",
    "mips-unknown-linux-gnu",
    "mips64-unknown-linux-gnuabi64",
    "mips64el-unknown-linux-gnuabi64",
    "mipsel-unknown-linux-gnu",
    "powerpc-unknown-linux-gnu",
    "powerpc64-unknown-linux-gnu",
    "powerpc64le-unknown-linux-gnu",
    "s390x-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "x86_64-linux-android",
    "x86_64-unknown-freebsd",
    "x86_64-unknown-linux-gnu",
    "x86_64-unknown-linux-musl",
    "x86_64-unknown-netbsd",
];

static _PLATFORMS_EXE: &'static [&'static str] = &[
    "i686-pc-windows-gnu",
    "i686-pc-windows-msvc",
    "x86_64-pc-windows-gnu",
    "x86_64-pc-windows-msvc",
];

pub fn sync(mirror: &MirrorSection, rustup: &RustupSection) {
    eprintln!("{}", style("Syncing Rustup repositories...").bold());

    // Mirror rustup-init
    eprintln!("{} Syncing rustup-init files...", style("[1/4]").bold());

    // Mirror stable
    if rustup.keep_latest_stables != Some(0) {
        eprintln!("{} Syncing latest stable...", style("[2/4]").bold());
        // Clean old stables
    }

    // Mirror beta
    if rustup.keep_latest_betas != Some(0) {
        eprintln!("{} Syncing latest beta...", style("[3/4]").bold());
        // Clean old betas
    }

    // Mirror nightly
    if rustup.keep_latest_nightlies != Some(0) {
        eprintln!("{} Syncing latest nightly...", style("[4/4]").bold());
        // Clean old nightlies
    }

    eprintln!("{}", style("Syncing Rustup repositories complete!").bold());
}
