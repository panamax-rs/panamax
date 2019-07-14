// Note: These platforms should match https://github.com/rust-lang/rustup.rs#other-installation-methods

use crate::mirror::{MirrorSection, RustupSection, MirrorError};
use console::style;
use std::path::Path;
use scoped_threadpool::Pool;
use log::debug;
use std::{fs, io};
use sha2::{Sha256, Digest};
use std::fs::{File, create_dir_all};

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

static PLATFORMS_EXE: &'static [&'static str] = &[
    "i686-pc-windows-gnu",
    "i686-pc-windows-msvc",
    "x86_64-pc-windows-gnu",
    "x86_64-pc-windows-msvc",
];

pub fn download_string(from: &str) -> String {
    // TODO: Error handling
    reqwest::get(from).unwrap().text().unwrap()
}

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

pub fn download_and_create_dir(from: &str, to: &Path) -> Result<(), io::Error> {
    // TODO: Error handling
    let mut http_res = reqwest::get(from).unwrap();

    let mut f = create_file_create_dir(to)?;

    http_res.copy_to(&mut f).unwrap();

    Ok(())
}

pub fn file_sha256(path: &Path) -> Result<String, io::Error> {
    let mut file = File::open(path)?;
    let mut sha256 = Sha256::new();
    io::copy(&mut file, &mut sha256)?;
    Ok(format!("{:x}", sha256.result()))
}

pub fn sync_one_init(path: &Path, source: &str, platform: &str, is_exe: bool) {
    let local_path = if is_exe {
        path.join("rustup/dist").join(platform).join("rustup-init.exe")
    } else {
        path.join("rustup/dist").join(platform).join("rustup-init")
    };
    let local_sha256_path = local_path.with_extension("sha256");

    let source_url = if is_exe {
        format!("{}/rustup/dist/{}/rustup-init.exe", source, platform)
    } else {
        format!("{}/rustup/dist/{}/rustup-init", source, platform)
    };
    let source_sha256_url = format!("{}.sha256", source_url);

    debug!("Checking hash for platform {}", platform);
    let source_sha256 = download_string(&source_sha256_url);

    let do_download = if let Ok(local_sha256) = fs::read_to_string(&local_sha256_path) {
        if local_sha256 == source_sha256 {
            if let Ok(local_file_sha256) = file_sha256(&local_path) {
                //dbg!(&local_file_sha256, &local_sha256, &source_sha256);
                source_sha256[..local_file_sha256.len()] != local_file_sha256 // Download if sha256 doesn't match
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
        debug!("Downloading rustup-init file for {} from {} to {}", platform, &source_url, &local_path.display());
        download_and_create_dir(&source_url, &local_path);
        write_file_create_dir(&local_sha256_path, &source_sha256);
    }

}

pub fn sync_rustup_init(path: &Path, source: &str, threads: usize) -> Result<(), MirrorError> {
    let mut pool = Pool::new(threads as u32);

    pool.scoped(|scoped| {
        for platform in PLATFORMS {
            scoped.execute(move || {
                sync_one_init(path, source, platform, false);
            })
        }

        for platform in PLATFORMS_EXE {
            scoped.execute(move || {
                sync_one_init(path, source, platform, true);
            })
        }
    });

    Ok(())
}

pub fn sync(path: &Path, mirror: &MirrorSection, rustup: &RustupSection) -> Result<(), MirrorError> {
    eprintln!("{}", style("Syncing Rustup repositories...").bold());

    // Mirror rustup-init
    eprintln!("{} Syncing rustup-init files...", style("[1/4]").bold());
    sync_rustup_init(path, &rustup.source, mirror.download_threads)?;

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

    Ok(())
}
