// Note: These platforms should match https://github.com/rust-lang/rustup.rs#other-installation-methods

use crate::mirror::{MirrorError, MirrorSection, RustupSection};
use console::style;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use log::debug;
use scoped_threadpool::Pool;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{create_dir_all, File};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::{mpsc, Arc};
use std::time::Duration;
use std::{fs, io, thread, mem};
use std::io::{Write, Read, ErrorKind};

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

/// Download a URL and return it as a string.
pub fn download_string(from: &str) -> String {
    // TODO: Error handling
    reqwest::get(from).unwrap().text().unwrap()
}

/// Write a string to a file, creating directories if needed.
pub fn write_file_create_dir(path: &Path, contents: &str) -> Result<(), io::Error> {
    // TODO: Error handling
    let mut res = fs::write(path, contents);

    if let Err(e) = &res {
        if e.kind() == io::ErrorKind::NotFound {
            fs::create_dir_all(path.parent().unwrap());
            res = fs::write(path, contents);
        }
    }

    res
}

/// Create a file, creating directories if needed.
pub fn create_file_create_dir(path: &Path) -> Result<File, io::Error> {
    let mut file_res = File::create(path);
    if let Err(e) = &file_res {
        if e.kind() == io::ErrorKind::NotFound {
            fs::create_dir_all(path.parent().unwrap());
            file_res = File::create(path);
        }
    }

    file_res
}

/// Download a file to a path, creating directories if needed.
pub fn download_and_create_dir(from: &str, to: &Path) -> Result<(), io::Error> {
    // TODO: Error handling
    //debug!("Downloading {} to {}", from, to.display());
    let mut http_res = reqwest::get(from).unwrap();

    let mut f = create_file_create_dir(to)?;

    http_res.copy_to(&mut f).unwrap();

    Ok(())
}

/// Clone of the io::copy code, but with the buffer size changed to 64k
pub fn fast_copy<R: ?Sized, W: ?Sized>(reader: &mut R, writer: &mut W) -> io::Result<u64>
    where R: Read, W: Write
{
    let mut buf: [u8; 65536] = [0; 65536];

    let mut written = 0;
    loop {
        let len = match reader.read(&mut buf) {
            Ok(0) => return Ok(written),
            Ok(len) => len,
            Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        writer.write_all(&buf[..len])?;
        written += len as u64;
    }
}

/// Get the (lowercase hex) sha256 hash of a file.
pub fn file_sha256(path: &Path) -> Result<String, io::Error> {
    //dbg!("file_sha256");
    let mut file = File::open(path)?;
    let mut sha256 = Sha256::new();
    fast_copy(&mut file, &mut sha256)?;
    //dbg!("file_sha256 done");
    Ok(format!("{:x}", sha256.result()))
}

/// If a file doesn't match a provided sha256, download a url to a path.
pub fn download_with_sha256_str_verify(url: &str, path: &Path, remote_sha256: &str) {
    // TODO: Error handling
    /*debug!(
        "Verifying sha256 by string and downloading {} to {}",
        url,
        path.display()
    );*/

    let do_download = if let Ok(local_file_sha256) = file_sha256(path) {
        remote_sha256 != local_file_sha256
    } else {
        true
    };

    if do_download {
        download_and_create_dir(url, path);
    }
}

/// If an accompanying .sha256 file doesn't match or exist, download a url to a path.
pub fn download_with_sha256_verify(url: &str, path: &Path) {
    // TODO: Error handling
    /*debug!(
        "Verifying sha256 and downloading {} to {}",
        url,
        path.display()
    );*/
    let sha256_url = format!("{}.sha256", url);
    let sha256_path = {
        let mut new_path = path.as_os_str().to_os_string();
        new_path.push(".sha256");
        PathBuf::from(new_path)
    };

    let remote_sha256 = download_string(&sha256_url);

    let do_download = if let Ok(local_sha256) = fs::read_to_string(&sha256_path) {
        if local_sha256 == remote_sha256 {
            if let Ok(local_file_sha256) = file_sha256(&path) {
                remote_sha256[..local_file_sha256.len()] != local_file_sha256 // Download if sha256 doesn't match
            } else {
                true // Local file doesn't exist or couldn't be read, so try to download
            }
        } else {
            true // Local sha256 file doesn't match, so download
        }
    } else {
        true // Local sha256 file not found, so download
    };

    if do_download {
        write_file_create_dir(&sha256_path, &remote_sha256);
        download_and_create_dir(url, path);
    }
}

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

    download_with_sha256_verify(&source_url, &local_path);
}

/// Synchronize all rustup-init files.
pub fn sync_rustup_init(path: &Path, source: &str, threads: usize) -> Result<(), MirrorError> {
    let count = PLATFORMS.len() + PLATFORMS_EXE.len();

    let mut pool = Pool::new(threads as u32);

    let (sender, receiver) = mpsc::channel();
    let pb_thread = thread::spawn(move || {
        let pb = ProgressBar::new(count as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{prefix} {wide_bar} {pos}/{len} [{elapsed_precise}]")
                .progress_chars("█▉▊▋▌▍▎▏  "),
        );
        pb.set_prefix(&format!(
            "{} Syncing rustup-init files...",
            style("[1/4]").bold()
        ));
        pb.enable_steady_tick(500);
        pb.tick();
        for _ in 0..count {
            receiver.recv().unwrap();
            pb.inc(1);
            thread::sleep(Duration::from_millis(100));
        }
        pb.finish();
    });

    pool.scoped(|scoped| {
        for platform in PLATFORMS {
            let s = sender.clone();
            scoped.execute(move || {
                sync_one_init(path, source, platform, false);
                s.send(());
            })
        }

        for platform in PLATFORMS_EXE {
            let s = sender.clone();
            scoped.execute(move || {
                sync_one_init(path, source, platform, true);
                s.send(());
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
pub fn rustup_download_list(path: &Path) -> Vec<(String, String)> {
    // TODO: Error handling
    let channel_str = fs::read_to_string(path).unwrap();
    let channel: Channel = toml::from_str(&channel_str).unwrap();

    channel
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
        .collect()
}

pub fn sync_one_rustup_target(path: &Path, source: &str, url: &str, hash: &str) {
    let target_path = path.join(url[source.len()..].trim_start_matches("/"));
    download_with_sha256_str_verify(url, &target_path, hash);
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
    download_with_sha256_verify(&channel_url, &channel_path);

    // Download release file
    let release_url = format!("{}/rustup/release-{}.toml", source, channel);
    let release_path = path.join(format!("rustup/release-{}.toml", channel));
    download_and_create_dir(&release_url, &release_path);

    // Open toml file, find all files to download
    let downloads = rustup_download_list(&channel_path);

    // Download files
    let mut pool = Pool::new(threads as u32);

    let (sender, receiver) = mpsc::channel();
    let count = downloads.len();
    let pb_thread = thread::spawn(move || {
        let pb = ProgressBar::new(count as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{prefix} {wide_bar} {pos}/{len} [{elapsed_precise}]")
                .progress_chars("█▉▊▋▌▍▎▏  "),
        );
        pb.set_prefix(&prefix);
        pb.enable_steady_tick(500);
        pb.tick();
        for _ in 0..count {
            receiver.recv().unwrap();
            pb.inc(1);
            thread::sleep(Duration::from_millis(100));
        }
        pb.finish();
    });

    pool.scoped(|scoped| {
        for (url, hash) in &downloads {
            let s = sender.clone();
            scoped.execute(move || {
                sync_one_rustup_target(&path, &source, &url, &hash);
                s.send(());
            })
        }
    });

    pb_thread.join();
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
    sync_rustup_init(path, &rustup.source, mirror.download_threads)?;

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
