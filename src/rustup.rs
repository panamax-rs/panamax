use crate::download::{
    append_to_path, download, download_with_sha256_file, move_if_exists,
    move_if_exists_with_sha256, DownloadError,
};
use crate::mirror::{MirrorError, MirrorSection, RustupSection};
use crate::progress_bar::{progress_bar, ProgressBarMessage};
use console::style;
use scoped_threadpool::Pool;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{fs, io};

// Note: These platforms should match https://github.com/rust-lang/rustup.rs#other-installation-methods

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

quick_error! {
    #[derive(Debug)]
    pub enum SyncError {
        Io(err: io::Error) {
            from()
        }
        Download(err: DownloadError) {
            from()
        }
        Parse(err: toml::de::Error) {
            from()
        }
        FailedDownloads(count: usize) {}
    }
}

/// Synchronize one rustup-init file.
pub fn sync_one_init(
    path: &Path,
    source: &str,
    platform: &str,
    is_exe: bool,
    retries: usize,
) -> Result<(), DownloadError> {
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

    download_with_sha256_file(&source_url, &local_path, retries, false)?;

    Ok(())
}

/// Synchronize all rustup-init files.
pub fn sync_rustup_init(
    path: &Path,
    source: &str,
    prefix: String,
    threads: usize,
    retries: usize,
) -> Result<(), SyncError> {
    let count = PLATFORMS.len() + PLATFORMS_EXE.len();

    let (pb_thread, sender) = progress_bar(count, prefix);

    let errors_occurred = AtomicUsize::new(0);

    Pool::new(threads as u32).scoped(|scoped| {
        let error_occurred = &errors_occurred;
        for platform in PLATFORMS {
            let s = sender.clone();
            scoped.execute(move || {
                if let Err(e) = sync_one_init(path, source, platform, false, retries) {
                    s.send(ProgressBarMessage::Println(format!(
                        "Downloading {} failed: {:?}",
                        path.display(),
                        e
                    )))
                    .expect("Channel send should not fail");
                    error_occurred.fetch_add(1, Ordering::Release);
                }
                s.send(ProgressBarMessage::Increment)
                    .expect("Channel send should not fail");
            })
        }

        for platform in PLATFORMS_EXE {
            let s = sender.clone();
            scoped.execute(move || {
                if let Err(e) = sync_one_init(path, source, platform, true, retries) {
                    s.send(ProgressBarMessage::Println(format!(
                        "Downloading {} failed: {:?}",
                        path.display(),
                        e
                    )))
                    .expect("Channel send should not fail");
                    error_occurred.fetch_add(1, Ordering::Release);
                }
                s.send(ProgressBarMessage::Increment)
                    .expect("Channel send should not fail");
            })
        }
    });

    pb_thread.join().unwrap();

    let errors = errors_occurred.load(Ordering::Acquire);
    if errors == 0 {
        Ok(())
    } else {
        Err(SyncError::FailedDownloads(errors))
    }
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
pub fn rustup_download_list(path: &Path) -> Result<Vec<(String, String)>, SyncError> {
    let channel_str = fs::read_to_string(path).map_err(|e| DownloadError::Io(e))?;
    let channel: Channel = toml::from_str(&channel_str)?;

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

pub fn sync_one_rustup_target(
    path: &Path,
    source: &str,
    url: &str,
    hash: &str,
    retries: usize,
) -> Result<(), DownloadError> {
    // Chop off the source portion of the URL, to mimic the rest of the path
    let target_path = path.join(url[source.len()..].trim_start_matches("/"));
    download(url, &target_path, Some(hash), retries, false)?;
    Ok(())
}

/// Synchronize a rustup channel (stable, beta, or nightly).
pub fn sync_rustup_channel(
    path: &Path,
    source: &str,
    threads: usize,
    prefix: String,
    channel: &str,
    retries: usize,
) -> Result<(), SyncError> {
    // Download channel file
    let channel_url = format!("{}/dist/channel-rust-{}.toml", source, channel);
    let channel_path = path.join(format!("dist/channel-rust-{}.toml", channel));
    let channel_part_path = append_to_path(&channel_path, ".part");
    download_with_sha256_file(&channel_url, &channel_part_path, retries, true)?;

    // Download release file
    let release_url = format!("{}/rustup/release-{}.toml", source, channel);
    let release_path = path.join(format!("rustup/release-{}.toml", channel));
    let release_part_path = append_to_path(&release_path, ".part");
    download(&release_url, &release_part_path, None, retries, false)?;

    // Open toml file, find all files to download
    let downloads = rustup_download_list(&channel_part_path)?;

    // Create progress bar
    let (pb_thread, sender) = progress_bar(downloads.len(), prefix);

    let errors_occurred = AtomicUsize::new(0);

    // Download files
    Pool::new(threads as u32).scoped(|scoped| {
        let error_occurred = &errors_occurred;
        for (url, hash) in &downloads {
            let s = sender.clone();
            scoped.execute(move || {
                if let Err(e) = sync_one_rustup_target(&path, &source, &url, &hash, retries) {
                    s.send(ProgressBarMessage::Println(format!(
                        "Downloading {} failed: {:?}",
                        path.display(),
                        e
                    )))
                    .expect("Channel send should not fail");
                    error_occurred.fetch_add(1, Ordering::Release);
                }
                s.send(ProgressBarMessage::Increment)
                    .expect("Channel send should not fail");
            })
        }
    });

    // Wait for progress bar to finish
    pb_thread.join().unwrap();

    let errors = errors_occurred.load(Ordering::Acquire);
    if errors == 0 {
        move_if_exists_with_sha256(&channel_part_path, &channel_path)?;
        move_if_exists(&release_part_path, &release_path)?;
        Ok(())
    } else {
        Err(SyncError::FailedDownloads(errors))
    }
}

/// Synchronize rustup.
pub fn sync(
    path: &Path,
    mirror: &MirrorSection,
    rustup: &RustupSection,
) -> Result<(), MirrorError> {
    eprintln!("{}", style("Syncing Rustup repositories...").bold());

    // Mirror rustup-init
    let prefix = format!("{} Syncing rustup-init files...", style("[1/4]").bold());
    if let Err(e) = sync_rustup_init(
        path,
        &rustup.source,
        prefix,
        mirror.download_threads,
        mirror.retries,
    ) {
        eprintln!("Downloading rustup init files failed: {:?}", e);
        eprintln!("You will need to sync again to finish this download.");
    }

    // Mirror stable
    if rustup.keep_latest_stables != Some(0) {
        let prefix = format!("{} Syncing latest stable...    ", style("[2/4]").bold());
        match sync_rustup_channel(
            path,
            &rustup.source,
            mirror.download_threads,
            prefix,
            "stable",
            mirror.retries,
        ) {
            Ok(()) => {
                // Clean old stables
            }
            Err(e) => {
                eprintln!("Downloading stable release failed: {:?}", e);
                eprintln!("You will need to sync again to finish this download.");
            }
        }
    }

    // Mirror beta
    if rustup.keep_latest_betas != Some(0) {
        let prefix = format!("{} Syncing latest beta...      ", style("[3/4]").bold());
        match sync_rustup_channel(
            path,
            &rustup.source,
            mirror.download_threads,
            prefix,
            "beta",
            mirror.retries,
        ) {
            Ok(()) => {
                // Clean old betas
            }
            Err(e) => {
                eprintln!("Downloading beta release failed: {:?}", e);
                eprintln!("You will need to sync again to finish this download.");
            }
        }
    }

    // Mirror nightly
    if rustup.keep_latest_nightlies != Some(0) {
        let prefix = format!("{} Syncing latest nightly...   ", style("[4/4]").bold());
        match sync_rustup_channel(
            path,
            &rustup.source,
            mirror.download_threads,
            prefix,
            "nightly",
            mirror.retries,
        ) {
            Ok(()) => {
                // Clean old nightlies
            }
            Err(e) => {
                eprintln!("Downloading nightly release failed: {:?}", e);
                eprintln!("You will need to sync again to finish this download.");
            }
        }
    }

    eprintln!("{}", style("Syncing Rustup repositories complete!").bold());

    Ok(())
}
