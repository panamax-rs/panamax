use crate::crates_index::{fast_forward, IndexSyncError};
use crate::download::{download, DownloadError};
use crate::mirror::{ConfigCrates, ConfigMirror};
use crate::progress_bar::padded_prefix_message;
use futures::StreamExt;
use git2::Repository;
use indicatif::{ProgressBar, ProgressFinish, ProgressStyle};
use reqwest::Client;
use reqwest::header::HeaderValue;
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fs::read_dir;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{
    fs,
    io::{self, BufRead, Cursor},
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SyncError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Download error: {0}")]
    Download(#[from] DownloadError),

    #[error("JSON serialization error: {0}")]
    SerializeError(#[from] serde_json::Error),

    #[error("Git error: {0}")]
    GitError(#[from] git2::Error),

    #[error("Index syncing error: {0}")]
    IndexSync(#[from] IndexSyncError),
}
/// One entry found in a crates.io-index file.
/// These files are formatted as lines of JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrateEntry {
    name: String,
    vers: String,
    cksum: String,
    yanked: bool,
}

impl CrateEntry {
    pub(crate) fn get_name(&self) -> &str {
        self.name.as_str()
    }

    pub(crate) fn get_vers(&self) -> &str {
        self.vers.as_str()
    }

    pub(crate) fn get_yanked(&self) -> bool {
        self.yanked
    }
}

/// Download one single crate file.
pub async fn sync_one_crate_entry(
    client: &Client,
    path: &Path,
    source: Option<&str>,
    retries: usize,
    crate_entry: &CrateEntry,
    user_agent: &HeaderValue,
) -> Result<(), DownloadError> {
    // If source is "https://crates.io/api/v1/crates" (the default, and thus a None here)
    // download straight from the static.crates.io CDN, to avoid bogging down crates.io itself
    // or affecting its statistics, and avoiding an extra redirect for each crate.
    let url = if let Some(source) = source {
        format!(
            "{}/{}/{}/download",
            source, crate_entry.name, crate_entry.vers
        )
    } else {
        format!(
            "https://static.crates.io/crates/{}/{}-{}.crate",
            crate_entry.name, crate_entry.name, crate_entry.vers
        )
    };

    let file_path = get_crate_path(path, &crate_entry.name, &crate_entry.vers)
        .ok_or_else(|| DownloadError::BadCrate(crate_entry.name.clone()))?;

    download(
        client,
        &url[..],
        &file_path,
        Some(&crate_entry.cksum),
        retries,
        false,
        user_agent,
    )
    .await
}

/// Synchronize the crate files themselves, using the index for a list of files.
// TODO: There are still many unwraps in the foreach sections. This needs to be fixed.
pub async fn sync_crates_files(
    path: &Path,
    vendor_path: Option<PathBuf>,
    mirror: &ConfigMirror,
    crates: &ConfigCrates,
    user_agent: &HeaderValue,
) -> Result<(), SyncError> {
    // if a vendor_path, parse the filepath for Cargo.toml files for each crate, filling vendors
    let mut vendors = vec![];
    if let Some(vendor_path) = &vendor_path {
        use walkdir::WalkDir;
        for entry in WalkDir::new(vendor_path.as_path())
            .min_depth(1)
            .max_depth(2)
        {
            let path = entry.as_ref().unwrap().path();
            if path.file_name() == Some(OsStr::new("Cargo.toml")) {
                let s = fs::read_to_string(entry.unwrap().path()).unwrap();
                let crate_toml = s.parse::<toml_edit::easy::Value>().unwrap();
                if let toml_edit::easy::Value::Table(crate_f) = crate_toml {
                    let name = crate_f["package"]["name"].to_string().replace('\"', "");
                    let version = crate_f["package"]["version"].to_string().replace('\"', "");
                    vendors.push((name, version));
                }
            }
        }
    }

    let prefix = padded_prefix_message(2, 3, "Syncing crates files");

    // For now, assume successful crates.io-index download
    let repo_path = path.join("crates.io-index");
    let repo = Repository::open(&repo_path)?;

    // Set the crates.io URL, or None if default
    let crates_source = if crates.source == "https://crates.io/api/v1/crates" {
        None
    } else {
        Some(crates.source.as_str())
    };

    // Find Reference for origin/master
    let origin_master = repo.find_reference("refs/remotes/origin/master")?;
    let origin_master_tree = origin_master.peel_to_tree()?;

    let master = repo.find_reference("refs/heads/master").ok();
    let master_tree = master.as_ref().and_then(|m| m.peel_to_tree().ok());

    // Diff between master and origin/master (i.e. everything since the last fetch)
    let diff = repo.diff_tree_to_tree(master_tree.as_ref(), Some(&origin_master_tree), None)?;

    let mut changed_crates = Vec::new();
    let mut removed_crates = Vec::new();

    let pb = ProgressBar::new_spinner()
        .with_style(
            ProgressStyle::default_bar()
                .template("{prefix} {wide_bar} {spinner} [{elapsed_precise}]")
                .expect("template is correct")
                .progress_chars("  "),
        )
        .with_finish(ProgressFinish::AndLeave)
        .with_prefix(prefix.clone());
    pb.enable_steady_tick(Duration::from_millis(10));

    // Figure out which crates we need to update/remove.
    diff.foreach(
        &mut |delta, _| {
            let df = delta.new_file();
            let p = df.path().unwrap();
            if p == Path::new("config.json") {
                return true;
            }
            if p.starts_with(".github/") {
                return true;
            }

            // DEV: if dev_reduced_crates is enabled, only download crates that start with z.
            // Keep this code in here, because it's helpful for development and debugging.
            #[cfg(feature = "dev_reduced_crates")]
            {
                // Get file name, try-convert to string, check if starts_with z, unwrap, or false if None
                if !p
                    .file_name()
                    .and_then(|x| x.to_str())
                    .map(|x| x.starts_with('z'))
                    .unwrap_or(false)
                {
                    return true;
                }
            }

            // Get the data for this crate file
            let oid = df.id();
            if oid.is_zero() {
                // The crate was removed, continue to next crate.
                // Note that this does not include yanked crates.
                removed_crates.push(p.to_path_buf());
                return true;
            }
            let blob = repo.find_blob(oid).unwrap();
            let data = blob.content();

            // Download one crate for each of the versions in the crate file
            for line in Cursor::new(data).lines() {
                let line = line.unwrap();
                let c = match serde_json::from_str::<CrateEntry>(&line) {
                    Ok(c) => {
                        // if vendor_path, check for matching crate name/version
                        if vendor_path.is_some() {
                            if vendors
                                .iter()
                                .any(|a| a == &(c.name.clone(), c.vers.clone()))
                            {
                                c
                            } else {
                                continue;
                            }
                        } else {
                            c
                        }
                    }
                    Err(_) => {
                        continue;
                    }
                };

                changed_crates.push(c);
            }

            true
        },
        None,
        None,
        None,
    )
    .unwrap();

    pb.finish_and_clear();
    let pb = ProgressBar::new(changed_crates.len() as u64)
        .with_style(
            ProgressStyle::default_bar()
                .template(
                    "{prefix} {wide_bar} {pos}/{len} [{elapsed_precise} / {duration_precise}]",
                )
                .expect("template is correct")
                .progress_chars("█▉▊▋▌▍▎▏  "),
        )
        .with_finish(ProgressFinish::AndLeave)
        .with_prefix(prefix);
    pb.enable_steady_tick(Duration::from_millis(10));

    let client = Client::new();

    let tasks = futures::stream::iter(changed_crates.into_iter())
        .map(|c| {
            let client = client.clone();
            // Duplicate variables used in the async closure.
            let path = path.to_owned();
            let mirror_retries = mirror.retries;
            let crates_source = crates_source.map(|s| s.to_string());
            let user_agent = user_agent.to_owned();
            let pb = pb.clone();

            tokio::spawn(async move {
                let out = sync_one_crate_entry(
                    &client,
                    &path,
                    crates_source.as_deref(),
                    mirror_retries,
                    &c,
                    &user_agent,
                )
                .await;

                pb.inc(1);

                out
            })
        })
        .buffer_unordered(crates.download_threads)
        .collect::<Vec<_>>()
        .await;

    for t in tasks {
        let res = t.unwrap();
        match res {
            Ok(())
            | Err(DownloadError::NotFound {
                status: _,
                url: _,
                data: _,
            })
            | Err(DownloadError::MismatchedHash {
                expected: _,
                actual: _,
            }) => {}

            Err(e) => {
                eprintln!("Downloading failed: {e:?}");
            }
        }
    }

    // Delete any removed crates
    for rc in removed_crates {
        // Try to remove the file, but ignore it if it doesn't exist
        let _ = fs::remove_file(repo_path.join(rc));
    }

    // Set master to origin/master.
    //
    // Note that this means config.json changes will have to be rewritten on every sync.
    fast_forward(&repo_path)?;

    Ok(())
}

/// Detect if the crates directory is using the old format.
pub fn is_new_crates_format(path: &Path) -> Result<bool, io::Error> {
    if !path.exists() {
        // Path doesn't exist, so we can start with a clean slate.
        return Ok(true);
    }

    for crate_dir in read_dir(path)? {
        let crate_dir = crate_dir?;
        if !crate_dir.file_type()?.is_dir() {
            // Ignore any files in the directory. Only look at other directories.
            continue;
        }

        let dir_name = crate_dir
            .file_name()
            .into_string()
            .map_err(|_| io::ErrorKind::Other)?;
        match dir_name.as_str() {
            // 1-letter crate names cannot be numbers, so this must be new format.
            "1" | "2" | "3" => continue,
            // 2-letter directories are used for crates longer than 3 characters.
            x if x.len() == 2 => continue,
            // Unrecognized directory found, might be crate in old format.
            _ => {
                return Ok(false);
            }
        };
    }

    Ok(true)
}

pub fn get_crate_path(
    mirror_path: &Path,
    crate_name: &str,
    crate_version: &str,
) -> Option<PathBuf> {
    let crate_path = match crate_name.len() {
        1 => PathBuf::from("1"),
        2 => PathBuf::from("2"),
        3 => {
            let first_char = crate_name.get(0..1)?;
            PathBuf::from("3").join(first_char)
        }
        n if n >= 4 => {
            let first_two = crate_name.get(0..2)?;
            let second_two = crate_name.get(2..4)?;
            [first_two, second_two].iter().collect()
        }
        _ => return None,
    };

    Some(
        mirror_path
            .join("crates")
            .join(crate_path)
            .join(crate_name)
            .join(crate_version)
            .join(format!("{crate_name}-{crate_version}.crate")),
    )
}
