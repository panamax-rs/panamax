use crate::download::{
    append_to_path, copy_file_create_dir_with_sha256, download, download_string,
    download_with_sha256_file, move_if_exists, move_if_exists_with_sha256, write_file_create_dir,
    DownloadError,
};
use crate::mirror::{ConfigMirror, ConfigRustup, MirrorError};
use crate::progress_bar::{current_step_prefix, padded_prefix_message};
use console::style;
use futures::StreamExt;
use indicatif::{ProgressBar, ProgressFinish, ProgressStyle};
use reqwest::header::HeaderValue;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{fs, io};
use thiserror::Error;
use tokio::task::JoinError;

// The allowed platforms to validate the configuration
// Note: These platforms should match the list on https://rust-lang.github.io/rustup/installation/other.html

/// Windows platforms (platforms where rustup-init has a .exe extension)
static PLATFORMS_WINDOWS: &[&str] = &[
    "i586-pc-windows-msvc",
    "i686-pc-windows-gnu",
    "i686-pc-windows-msvc",
    "x86_64-pc-windows-gnu",
    "x86_64-pc-windows-msvc",
];

#[derive(Error, Debug)]
pub enum SyncError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("Download error: {0}")]
    Download(#[from] DownloadError),
    #[error("TOML deserialization error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("TOML serialization error: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("Path prefix strip error: {0}")]
    StripPrefix(#[from] std::path::StripPrefixError),
    #[error("Failed {count} downloads")]
    FailedDownloads { count: usize },
}

#[derive(Deserialize, Debug)]
pub struct TargetUrls {
    pub url: String,
    pub hash: String,
    pub xz_url: String,
    pub xz_hash: String,
}

#[derive(Deserialize, Debug)]
pub struct Target {
    pub available: bool,

    #[serde(flatten)]
    pub target_urls: Option<TargetUrls>,
}

#[derive(Deserialize, Debug)]
pub struct Pkg {
    pub version: String,
    pub target: HashMap<String, Target>,
}

#[derive(Deserialize, Debug)]
pub struct Channel {
    #[serde(alias = "manifest-version")]
    pub manifest_version: String,
    pub date: String,
    pub pkg: HashMap<String, Pkg>,
}

#[derive(Deserialize, Debug)]
struct Release {
    version: String,
}

#[derive(Deserialize, Debug)]
pub struct Platforms {
    unix: Vec<String>,
    windows: Vec<String>,
}

impl Platforms {
    // &String instead of &str is required due to vec.contains not performing proper inference
    // here. See:
    // https://stackoverflow.com/questions/48985924/why-does-a-str-not-coerce-to-a-string-when-using-veccontains
    // https://github.com/rust-lang/rust/issues/42671
    #[allow(clippy::ptr_arg)]
    pub fn contains(&self, platform: &String) -> bool {
        self.unix.contains(platform) || self.windows.contains(platform)
    }

    pub fn len(&self) -> usize {
        self.unix.len() + self.windows.len()
    }
}

pub async fn download_platform_list(
    source: &str,
    channel: &str,
) -> Result<Vec<String>, MirrorError> {
    let channel_url = format!("{source}/dist/channel-rust-{channel}.toml");
    let user_agent = HeaderValue::from_str(&format!("Panamax/{}", env!("CARGO_PKG_VERSION")))
        .expect("Hardcoded user agent string should never fail.");
    let channel_str = download_string(&channel_url, &user_agent).await?;
    let channel_data: Channel = toml::from_str(&channel_str)?;

    let mut targets = HashSet::new();

    for (_, pkg) in channel_data.pkg {
        for (target, _) in pkg.target {
            if target == "*" {
                continue;
            }
            targets.insert(target);
        }
    }

    let mut targets: Vec<String> = targets.into_iter().collect();
    targets.sort();

    Ok(targets)
}

pub async fn get_platforms(rustup: &ConfigRustup) -> Result<Platforms, MirrorError> {
    let all = download_platform_list(&rustup.source, "nightly").await?;

    let unix = all
        .iter()
        .filter(|x| !PLATFORMS_WINDOWS.contains(&x.as_str()))
        .map(|x| x.to_string())
        .collect();

    let windows = match &rustup.platforms_windows {
        Some(p) => p.clone(),
        None => PLATFORMS_WINDOWS.iter().map(|x| x.to_string()).collect(),
    };
    Ok(Platforms { unix, windows })
}

/// Synchronize one rustup-init file.
pub async fn sync_one_init(
    path: &Path,
    source: &str,
    platform: &str,
    is_exe: bool,
    rustup_version: &str,
    retries: usize,
    user_agent: &HeaderValue,
) -> Result<(), DownloadError> {
    let local_path = path
        .join("rustup")
        .join("archive")
        .join(rustup_version)
        .join(platform)
        .join(if is_exe {
            "rustup-init.exe"
        } else {
            "rustup-init"
        });

    let archive_path = path.join("rustup/dist").join(platform).join(if is_exe {
        "rustup-init.exe"
    } else {
        "rustup-init"
    });

    let source_url = if is_exe {
        format!("{source}/rustup/dist/{platform}/rustup-init.exe")
    } else {
        format!("{source}/rustup/dist/{platform}/rustup-init")
    };

    download_with_sha256_file(&source_url, &local_path, retries, false, user_agent).await?;
    copy_file_create_dir_with_sha256(&local_path, &archive_path)?;

    Ok(())
}

fn panamax_progress_bar(size: usize, prefix: String) -> ProgressBar {
    ProgressBar::new(size as u64)
        .with_style(
            ProgressStyle::default_bar()
                .template(
                    "{prefix} {wide_bar} {pos}/{len} [{elapsed_precise} / {duration_precise}]",
                )
                .expect("template is correct")
                .progress_chars("█▉▊▋▌▍▎▏  "),
        )
        .with_finish(ProgressFinish::AndLeave)
        .with_prefix(prefix)
}

#[allow(clippy::too_many_arguments)]
async fn create_sync_tasks(
    platforms: &[String],
    is_exe: bool,
    rustup_version: &str,
    path: &Path,
    source: &str,
    retries: usize,
    user_agent: &HeaderValue,
    threads: usize,
    pb: &ProgressBar,
) -> Vec<Result<Result<(), DownloadError>, JoinError>> {
    futures::stream::iter(platforms.iter())
        .map(|platform| {
            let rustup_version = rustup_version.to_string();
            let path = path.to_path_buf();
            let source = source.to_string();
            let user_agent = user_agent.clone();
            let platform = platform.clone();
            let pb = pb.clone();

            tokio::spawn(async move {
                let out = sync_one_init(
                    &path,
                    &source,
                    platform.as_str(),
                    is_exe,
                    &rustup_version,
                    retries,
                    &user_agent,
                )
                .await;

                pb.inc(1);

                out
            })
        })
        .buffer_unordered(threads)
        .collect::<Vec<Result<_, _>>>()
        .await
}

/// Synchronize all rustup-init files.
pub async fn sync_rustup_init(
    path: &Path,
    threads: usize,
    source: &str,
    prefix: String,
    retries: usize,
    user_agent: &HeaderValue,
    platforms: &Platforms,
) -> Result<(), SyncError> {
    let mut errors_occurred = 0usize;

    // Download rustup release file
    let release_url = format!("{source}/rustup/release-stable.toml");
    let release_path = path.join("rustup/release-stable.toml");
    let release_part_path = append_to_path(&release_path, ".part");

    download(
        &release_url,
        &release_part_path,
        None,
        retries,
        false,
        user_agent,
    )
    .await?;

    let rustup_version = get_rustup_version(&release_part_path)?;

    move_if_exists(&release_part_path, &release_path)?;

    let pb = panamax_progress_bar(platforms.len(), prefix);
    pb.enable_steady_tick(Duration::from_millis(10));

    let unix_tasks = create_sync_tasks(
        &platforms.unix,
        false,
        &rustup_version,
        path,
        source,
        retries,
        user_agent,
        threads,
        &pb,
    )
    .await;

    let win_tasks = create_sync_tasks(
        &platforms.windows,
        true,
        &rustup_version,
        path,
        source,
        retries,
        user_agent,
        threads,
        &pb,
    )
    .await;

    for res in unix_tasks.into_iter().chain(win_tasks) {
        // Unwrap the join result.
        let res = res.unwrap();

        if let Err(e) = res {
            match e {
                DownloadError::NotFound { .. } => {}
                _ => {
                    errors_occurred += 1;
                    eprintln!("Download failed: {e:?}");
                }
            }
        }
    }

    if errors_occurred == 0 {
        Ok(())
    } else {
        Err(SyncError::FailedDownloads {
            count: errors_occurred,
        })
    }
}

/// Get the rustup file downloads, in pairs of URLs and sha256 hashes.
pub fn rustup_download_list(
    path: &Path,
    download_dev: bool,
    download_gz: bool,
    download_xz: bool,
    platforms: &Platforms,
) -> Result<(String, Vec<(String, String)>), SyncError> {
    let channel_str = fs::read_to_string(path).map_err(DownloadError::Io)?;
    let channel: Channel = toml::from_str(&channel_str)?;

    Ok((
        channel.date,
        channel
            .pkg
            .into_iter()
            .filter(|(pkg_name, _)| download_dev || pkg_name != "rustc-dev")
            .flat_map(|(_, pkg)| {
                pkg.target
                    .into_iter()
                    .filter(
                        |(name, _)| platforms.contains(name) || name == "*", // The * platform contains rust-src, always download
                    )
                    .flat_map(|(_, target)| -> Vec<(String, String)> {
                        target
                            .target_urls
                            .map(|urls| {
                                let mut v = Vec::new();
                                if download_gz {
                                    v.push((urls.url, urls.hash));
                                }
                                if download_xz {
                                    v.push((urls.xz_url, urls.xz_hash));
                                }

                                v
                            })
                            .into_iter()
                            .flatten()
                            .map(|(url, hash)| {
                                (url.split('/').collect::<Vec<&str>>()[3..].join("/"), hash)
                            })
                            .collect()
                    })
            })
            .collect(),
    ))
}

pub async fn sync_one_rustup_target(
    path: &Path,
    source: &str,
    url: &str,
    hash: &str,
    retries: usize,
    user_agent: &HeaderValue,
) -> Result<(), DownloadError> {
    // Chop off the source portion of the URL, to mimic the rest of the path
    //let target_url = path.join(url[source.len()..].trim_start_matches("/"));
    let target_url = format!("{source}/{url}");
    let target_path: PathBuf = std::iter::once(path.to_owned())
        .chain(url.split('/').map(PathBuf::from))
        .collect();

    download(
        &target_url,
        &target_path,
        Some(hash),
        retries,
        false,
        user_agent,
    )
    .await
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChannelHistoryFile {
    pub versions: HashMap<String, Vec<String>>,
}

pub fn latest_dates_from_channel_history(
    channel_history: &ChannelHistoryFile,
    versions: usize,
) -> Vec<String> {
    let mut dates: Vec<String> = channel_history
        .versions
        .keys()
        .map(|x| x.to_string())
        .collect();
    dates.sort();
    dates.reverse();
    dates.truncate(versions);
    dates
}

pub fn clean_old_files(
    path: &Path,
    keep_stables: Option<usize>,
    keep_betas: Option<usize>,
    keep_nightlies: Option<usize>,
    pinned_rust_versions: Option<&Vec<String>>,
    prefix: String,
) -> Result<(), SyncError> {
    let versions = [
        ("stable", keep_stables),
        ("beta", keep_betas),
        ("nightly", keep_nightlies),
    ];

    // Handle all of stable/beta/nightly
    let mut files_to_keep: HashSet<PathBuf> = HashSet::new();
    for (channel, keep_version) in versions {
        if let Some(s) = keep_version {
            let mut history = match get_channel_history(path, channel) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let latest_dates = latest_dates_from_channel_history(&history, s);
            for date in latest_dates {
                if let Some(t) = history.versions.get_mut(&date) {
                    t.iter().for_each(|t| {
                        // Convert the path to a PathBuf.
                        let path: PathBuf = t.split('/').collect();
                        files_to_keep.insert(path);
                    });
                }
            }
        }
    }

    if let Some(pinned_versions) = pinned_rust_versions {
        for version in pinned_versions {
            let mut pinned = match get_channel_history(path, version) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let latest_dates = latest_dates_from_channel_history(&pinned, 1);
            for date in latest_dates {
                if let Some(t) = pinned.versions.get_mut(&date) {
                    t.iter().for_each(|t| {
                        // Convert the path to a PathBuf.
                        let path: PathBuf = t.split('/').collect();

                        files_to_keep.insert(path);
                    });
                }
            }
        }
    }

    let dist_path = path.join("dist");
    let mut files_to_delete = Vec::new();

    for dir in fs::read_dir(dist_path)? {
        let dir = dir?.path();
        if dir.is_dir() {
            for full_path in fs::read_dir(dir)? {
                let full_path = full_path?.path();
                let file_path = full_path.strip_prefix(path)?;

                if !files_to_keep.contains(file_path) {
                    files_to_delete.push(file_path.to_owned());
                }
            }
        }
    }

    // Progress bar!
    let pb = panamax_progress_bar(files_to_delete.len(), prefix);

    for f in files_to_delete {
        if let Err(e) = fs::remove_file(path.join(&f)) {
            eprintln!("Could not remove file {}: {:?}", f.to_string_lossy(), e);
        }
        pb.inc(1);
    }

    Ok(())
}

pub fn get_channel_history(path: &Path, channel: &str) -> Result<ChannelHistoryFile, SyncError> {
    let channel_history_path = path.join(format!("mirror-{channel}-history.toml"));
    let ch_data = fs::read_to_string(channel_history_path)?;
    Ok(toml::from_str(&ch_data)?)
}

pub fn add_to_channel_history(
    path: &Path,
    channel: &str,
    date: &str,
    files: &[(String, String)],
    extra_files: &[String],
) -> Result<(), SyncError> {
    let mut channel_history = match get_channel_history(path, channel) {
        Ok(c) => c,
        Err(SyncError::Io(_)) => ChannelHistoryFile {
            versions: HashMap::new(),
        },
        Err(e) => return Err(e),
    };

    let files = files.iter().map(|(f, _)| f.to_string());
    let extra_files = extra_files.iter().map(|ef| ef.to_string());

    let files = files.chain(extra_files).collect();

    channel_history.versions.insert(date.to_string(), files);

    let ch_data = toml::to_string(&channel_history)?;

    let channel_history_path = path.join(format!("mirror-{channel}-history.toml"));
    write_file_create_dir(&channel_history_path, &ch_data)?;

    Ok(())
}

/// Get the current rustup version from release-stable.toml.
pub fn get_rustup_version(path: &Path) -> Result<String, SyncError> {
    let release_data: Release = toml::from_str(&fs::read_to_string(path)?)?;
    Ok(release_data.version)
}

/// Synchronize a rustup channel (stable, beta, or nightly).
#[allow(clippy::too_many_arguments)]
pub async fn sync_rustup_channel(
    path: &Path,
    source: &str,
    threads: usize,
    prefix: String,
    channel: &str,
    retries: usize,
    user_agent: &HeaderValue,
    download_dev: bool,
    download_gz: bool,
    download_xz: bool,
    platforms: &Platforms,
) -> Result<(), SyncError> {
    // Download channel file
    let (channel_url, channel_path, extra_files) =
        if let Some(inner_channel) = channel.strip_prefix("nightly-") {
            let url = format!("{source}/dist/{inner_channel}/channel-rust-nightly.toml");
            let path_chunk = format!("dist/{inner_channel}/channel-rust-nightly.toml");
            let path = path.join(&path_chunk);
            // Make sure the cleanup step doesn't delete the channel toml
            let extra_files = vec![path_chunk.clone(), format!("{path_chunk}.sha256")];
            (url, path, extra_files)
        } else {
            let url = format!("{source}/dist/channel-rust-{channel}.toml");
            let path = path.join(format!("dist/channel-rust-{channel}.toml"));
            (url, path, Vec::new())
        };
    let channel_part_path = append_to_path(&channel_path, ".part");
    download_with_sha256_file(&channel_url, &channel_part_path, retries, true, user_agent).await?;

    // Open toml file, find all files to download
    let (date, files) = rustup_download_list(
        &channel_part_path,
        download_dev,
        download_gz,
        download_xz,
        platforms,
    )?;
    move_if_exists_with_sha256(&channel_part_path, &channel_path)?;

    let pb = panamax_progress_bar(files.len(), prefix);
    pb.enable_steady_tick(Duration::from_millis(10));

    let mut errors_occurred = 0usize;

    let tasks = futures::stream::iter(files.iter())
        .map(|(url, hash)| {
            // Clone the variables that will be moved into the tokio task.
            let path = path.to_path_buf();
            let source = source.to_string();
            let user_agent = user_agent.clone();
            let url = url.clone();
            let hash = hash.clone();
            let pb = pb.clone();

            tokio::spawn(async move {
                let out =
                    sync_one_rustup_target(&path, &source, &url, &hash, retries, &user_agent).await;

                pb.inc(1);

                out
            })
        })
        .buffer_unordered(threads)
        .collect::<Vec<_>>()
        .await;

    for res in tasks {
        // Unwrap the join result.
        let res = res.unwrap();

        if let Err(e) = res {
            match e {
                DownloadError::NotFound { .. } => {}
                _ => {
                    errors_occurred += 1;
                    eprintln!("Download failed: {e:?}");
                }
            }
        }
    }

    if errors_occurred == 0 {
        // Write channel history file
        add_to_channel_history(path, channel, &date, &files, &extra_files)?;
        Ok(())
    } else {
        Err(SyncError::FailedDownloads {
            count: errors_occurred,
        })
    }
}

/// Synchronize rustup.
pub async fn sync(
    path: &Path,
    mirror: &ConfigMirror,
    rustup: &ConfigRustup,
    user_agent: &HeaderValue,
) -> Result<(), MirrorError> {
    let platforms = get_platforms(rustup).await?;
    // Default to not downloading rustc-dev
    let download_dev = rustup.download_dev.unwrap_or(false);

    let download_gz = rustup.download_gz.unwrap_or(false);
    let download_xz = rustup.download_xz.unwrap_or(true);

    let num_pinned_versions = rustup.pinned_rust_versions.as_ref().map_or(0, |v| v.len());
    let num_steps = 1 + // sync rustup-init
                    1 + 1 + 1 + // sync latest stable, beta, nightly
                    num_pinned_versions + // sync pinned rust versions
                    1; // clean old files
    let mut step = 0;

    eprintln!("{}", style("Syncing Rustup repositories...").bold());

    // Mirror rustup-init
    step += 1;
    let prefix = padded_prefix_message(step, num_steps, "Syncing rustup-init files");
    if let Err(e) = sync_rustup_init(
        path,
        rustup.download_threads,
        &rustup.source,
        prefix,
        mirror.retries,
        user_agent,
        &platforms,
    )
    .await
    {
        eprintln!("Downloading rustup init files failed: {e:?}");
        eprintln!("You will need to sync again to finish this download.");
    }

    let mut failures = false;

    // Mirror stable
    step += 1;
    if rustup.keep_latest_stables != Some(0) {
        let prefix = padded_prefix_message(step, num_steps, "Syncing latest stable");
        if let Err(e) = sync_rustup_channel(
            path,
            &rustup.source,
            rustup.download_threads,
            prefix,
            "stable",
            mirror.retries,
            user_agent,
            download_dev,
            download_gz,
            download_xz,
            &platforms,
        )
        .await
        {
            failures = true;
            eprintln!("Downloading stable release failed: {e:?}");
            eprintln!("You will need to sync again to finish this download.");
        }
    } else {
        eprintln!(
            "{} Skipping syncing stable.",
            current_step_prefix(step, num_steps)
        );
    }

    // Mirror beta
    step += 1;
    if rustup.keep_latest_betas != Some(0) {
        let prefix = padded_prefix_message(step, num_steps, "Syncing latest beta");
        if let Err(e) = sync_rustup_channel(
            path,
            &rustup.source,
            rustup.download_threads,
            prefix,
            "beta",
            mirror.retries,
            user_agent,
            download_dev,
            download_gz,
            download_xz,
            &platforms,
        )
        .await
        {
            failures = true;
            eprintln!("Downloading beta release failed: {e:?}");
            eprintln!("You will need to sync again to finish this download.");
        }
    } else {
        eprintln!(
            "{} Skipping syncing beta.",
            current_step_prefix(step, num_steps)
        );
    }

    // Mirror nightly
    step += 1;
    if rustup.keep_latest_nightlies != Some(0) {
        let prefix = padded_prefix_message(step, num_steps, "Syncing latest nightly");
        if let Err(e) = sync_rustup_channel(
            path,
            &rustup.source,
            rustup.download_threads,
            prefix,
            "nightly",
            mirror.retries,
            user_agent,
            download_dev,
            download_gz,
            download_xz,
            &platforms,
        )
        .await
        {
            failures = true;
            eprintln!("Downloading nightly release failed: {e:?}");
            eprintln!("You will need to sync again to finish this download.");
        }
    } else {
        eprintln!(
            "{} Skipping syncing nightly.",
            current_step_prefix(step, num_steps)
        );
    }

    // Mirror pinned rust versions
    if let Some(pinned_versions) = &rustup.pinned_rust_versions {
        for version in pinned_versions {
            step += 1;
            let prefix =
                padded_prefix_message(step, num_steps, &format!("Syncing pinned rust {version}"));
            if let Err(e) = sync_rustup_channel(
                path,
                &rustup.source,
                rustup.download_threads,
                prefix,
                version,
                mirror.retries,
                user_agent,
                download_dev,
                download_gz,
                download_xz,
                &platforms,
            )
            .await
            {
                failures = true;
                if let SyncError::Download(DownloadError::NotFound { .. }) = e {
                    eprintln!(
                        "{} Pinned rust version {} could not be found.",
                        current_step_prefix(step, num_steps),
                        version
                    );
                    return Err(MirrorError::Config(format!(
                        "Pinned rust version {version} could not be found"
                    )));
                } else {
                    eprintln!("Downloading pinned rust {version} failed: {e:?}");
                    eprintln!("You will need to sync again to finish this download.");
                }
            }
        }
    }

    // If all succeeds, clean files
    step += 1;
    if rustup.keep_latest_stables.is_none()
        && rustup.keep_latest_betas.is_none()
        && rustup.keep_latest_nightlies.is_none()
    {
        eprintln!(
            "{} Skipping cleaning files.",
            current_step_prefix(step, num_steps)
        );
    } else if failures {
        eprintln!(
            "{} Skipping cleaning files due to download failures.",
            current_step_prefix(step, num_steps)
        );
    } else {
        let prefix = padded_prefix_message(step, num_steps, "Cleaning old files");
        if let Err(e) = clean_old_files(
            path,
            rustup.keep_latest_stables,
            rustup.keep_latest_betas,
            rustup.keep_latest_nightlies,
            rustup.pinned_rust_versions.as_ref(),
            prefix,
        ) {
            eprintln!("Cleaning old files failed: {e:?}");
            eprintln!("You may need to sync again to clean these files.");
        }
    }

    eprintln!("{}", style("Syncing Rustup repositories complete!").bold());

    Ok(())
}
