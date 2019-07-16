// Note: These platforms should match https://github.com/rust-lang/rustup.rs#other-installation-methods

use crate::mirror::{MirrorError, MirrorSection, RustupSection};
use console::style;
use scoped_threadpool::Pool;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::{fs, io};
use crate::progress_bar::{progress_bar, ProgressBarMessage};
use crate::download::{download_with_sha256_verify, download_with_sha256_str_verify, download_and_create_dir};

/// Non-windows platforms
static PLATFORMS: &'static [&'static str] = &[
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
    // "x86_64-unknown-linux-musl", // No .sha256 file, so disable rustup-init for this platform
    "x86_64-unknown-netbsd",
];

/// Windows platforms (platforms where rustup-init has a .exe extension)
static PLATFORMS_EXE: &'static [&'static str] = &[
    "i686-pc-windows-gnu",
    "i686-pc-windows-msvc",
    "x86_64-pc-windows-gnu",
    "x86_64-pc-windows-msvc",
];

/// Synchronize one rustup-init file.
pub fn sync_one_init(path: &Path, source: &str, platform: &str, is_exe: bool) {
    let local_path = if is_exe {
        path.join("rustup/dist")
            .join(platform)
            .join("rustup-init.exe")
    } else {
        path.join("rustup/dist").join(platform).join("rustup-init")
    };

    let source_url = if is_exe {
        format!("{}/rustup/dist/{}/rustup-init.exe", source, platform)
    } else {
        format!("{}/rustup/dist/{}/rustup-init", source, platform)
    };

    // TODO: error handling
    download_with_sha256_verify(&source_url, &local_path).unwrap();
}

/// Synchronize all rustup-init files.
pub fn sync_rustup_init(path: &Path, source: &str, prefix: String, threads: usize) -> Result<(), MirrorError> {
    let count = PLATFORMS.len() + PLATFORMS_EXE.len();

    let (pb_thread, sender) = progress_bar(count, prefix);

    Pool::new(threads as u32).scoped(|scoped| {
        for platform in PLATFORMS {
            let s = sender.clone();
            scoped.execute(move || {
                sync_one_init(path, source, platform, false);
                s.send(ProgressBarMessage::Increment).unwrap();
            })
        }

        for platform in PLATFORMS_EXE {
            let s = sender.clone();
            scoped.execute(move || {
                sync_one_init(path, source, platform, true);
                s.send(ProgressBarMessage::Increment).unwrap();
            })
        }
    });

    pb_thread.join().unwrap();

    Ok(())
}

#[derive(Deserialize, Debug)]
struct TargetUrls {
    url: String,
    hash: String,
    xz_url: String,
    xz_hash: String,
}

#[derive(Deserialize, Debug)]
struct Target {
    available: bool,

    #[serde(flatten)]
    target_urls: Option<TargetUrls>,
}

#[derive(Deserialize, Debug)]
struct Pkg {
    version: String,
    target: HashMap<String, Target>,
}

#[derive(Deserialize, Debug)]
struct Channel {
    #[serde(alias = "manifest-version")]
    manifest_version: String,
    date: String,
    pkg: HashMap<String, Pkg>,
}

/// Get the rustup file downloads, in pairs of URLs and sha256 hashes.
pub fn rustup_download_list(path: &Path) -> Result<Vec<(String, String)>, io::Error> {
    // TODO: Error handling
    let channel_str = fs::read_to_string(path)?;
    let channel: Channel = toml::from_str(&channel_str).unwrap();

    Ok(channel
        .pkg
        .into_iter()
        .flat_map(|(_, pkg)| {
            pkg.target
                .into_iter()
                .flat_map(|(_, target)| -> Vec<(String, String)> {
                    target
                        .target_urls
                        .map(|urls| vec![(urls.url, urls.hash), (urls.xz_url, urls.xz_hash)])
                        .into_iter()
                        .flatten()
                        .collect()
                })
        })
        .collect())
}

pub fn sync_one_rustup_target(path: &Path, source: &str, url: &str, hash: &str) {
    let target_path = path.join(url[source.len()..].trim_start_matches("/"));
    download_with_sha256_str_verify(url, &target_path, hash).unwrap();
}

/// Synchronize a rustup channel (stable, beta, or nightly).
pub fn sync_rustup_channel(
    path: &Path,
    source: &str,
    threads: usize,
    prefix: String,
    channel: &str,
) {
    // Download channel file
    let channel_url = format!("{}/dist/channel-rust-{}.toml", source, channel);
    let channel_path = path.join(format!("dist/channel-rust-{}.toml", channel));
    download_with_sha256_verify(&channel_url, &channel_path).unwrap();

    // Download release file
    let release_url = format!("{}/rustup/release-{}.toml", source, channel);
    let release_path = path.join(format!("rustup/release-{}.toml", channel));
    download_and_create_dir(&release_url, &release_path).unwrap();

    // Open toml file, find all files to download
    let downloads = rustup_download_list(&channel_path).unwrap();

    // Create progress bar
    let (pb_thread, sender) = progress_bar(downloads.len(), prefix);

    // Download files
    Pool::new(threads as u32).scoped(|scoped| {
        for (url, hash) in &downloads {
            let s = sender.clone();
            scoped.execute(move || {
                sync_one_rustup_target(&path, &source, &url, &hash);
                s.send(ProgressBarMessage::Increment).unwrap();
            })
        }
    });

    // Wait for progress bar to finish
    pb_thread.join().unwrap();
}

/// Synchronize rustup.
pub fn sync(
    path: &Path,
    mirror: &MirrorSection,
    rustup: &RustupSection,
) -> Result<(), MirrorError> {
    eprintln!("{}", style("Syncing Rustup repositories...").bold());

    // Mirror rustup-init
    //eprintln!("{} Syncing rustup-init files...", style("[1/4]").bold());
    let prefix = format!("{} Syncing rustup-init files...", style("[1/4]").bold());
    sync_rustup_init(path, &rustup.source, prefix, mirror.download_threads)?;

    // Mirror stable
    if rustup.keep_latest_stables != Some(0) {
        let prefix = format!("{} Syncing latest stable...    ", style("[2/4]").bold());
        sync_rustup_channel(
            path,
            &rustup.source,
            mirror.download_threads,
            prefix,
            "stable",
        );
        // Clean old stables
    }

    // Mirror beta
    if rustup.keep_latest_betas != Some(0) {
        let prefix = format!("{} Syncing latest beta...      ", style("[3/4]").bold());
        sync_rustup_channel(
            path,
            &rustup.source,
            mirror.download_threads,
            prefix,
            "beta",
        );
        // Clean old betas
    }

    // Mirror nightly
    if rustup.keep_latest_nightlies != Some(0) {
        let prefix = format!("{} Syncing latest nightly...   ", style("[4/4]").bold());
        sync_rustup_channel(
            path,
            &rustup.source,
            mirror.download_threads,
            prefix,
            "nightly",
        );
        // Clean old nightlies
    }

    eprintln!("{}", style("Syncing Rustup repositories complete!").bold());

    Ok(())
}
