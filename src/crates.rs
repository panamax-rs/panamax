use crate::download::{download, DownloadError};
use crate::mirror::{ConfigCrates, ConfigMirror};
use crate::progress_bar::{progress_bar, ProgressBarMessage};
use console::style;
use git2::Repository;
use reqwest::header::HeaderValue;
use scoped_threadpool::Pool;
use serde::{Deserialize, Serialize};
use std::path::Path;
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
}
/// One entry found in a crates.io-index file.
/// These files are formatted as lines of JSON.
#[derive(Debug, Serialize, Deserialize)]
pub struct CrateEntry {
    name: String,
    vers: String,
    cksum: String,
    yanked: bool,
}

/// Download one single crate file.
pub fn sync_one_crate_entry(
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

    let file_path = path
        .join("crates")
        .join(&crate_entry.name)
        .join(&crate_entry.vers)
        .join("download");

    download(
        &url[..],
        &file_path,
        Some(&crate_entry.cksum),
        retries,
        false,
        user_agent,
    )
}

/// Synchronize the crate files themselves, using the index for a list of files.
// TODO: There are still many unwraps in the foreach sections. This needs to be fixed.
pub fn sync_crates_files(
    path: &Path,
    mirror: &ConfigMirror,
    crates: &ConfigCrates,
    user_agent: &HeaderValue,
) -> Result<(), SyncError> {
    let prefix = format!("{} Syncing crates files...     ", style("[2/3]").bold());

    // For now, assume successful crates.io-index download
    let repo_path = path.join("crates.io-index");
    let repo = Repository::open(&repo_path)?;

    // Set the crates.io URL, or None if default
    let crates_source = if crates.source == "https://crates.io/api/v1/crates" {
        None
    } else {
        Some(crates.source.as_ref())
    };

    // Find Reference for origin/master
    let origin_master = repo.find_reference("refs/remotes/origin/master")?;
    let origin_master_tree = origin_master.peel_to_tree()?;

    let master = repo.find_reference("refs/heads/master")?;
    let master_tree = master.peel_to_tree()?;

    // Diff between master and origin/master (i.e. everything since the last fetch)
    let diff = repo.diff_tree_to_tree(Some(&master_tree), Some(&origin_master_tree), None)?;

    // Run one pass to figure out a total count
    let mut count = 0;
    diff.foreach(
        &mut |delta, _| {
            let df = delta.new_file();
            let p = df.path().unwrap();
            if p == Path::new("config.json") {
                // Skip config.json, as it's the only file that's not a crate descriptor
                return true;
            }
            let oid = df.id();
            if oid.is_zero() {
                // The crate was removed, continue to next crate
                return true;
            }
            let blob = repo.find_blob(oid).unwrap();
            let data = blob.content();
            count += Cursor::new(data).lines().count();
            true
        },
        None,
        None,
        None,
    )?;

    let (pb_thread, sender) = progress_bar(Some(count), prefix);

    let mut removed_crates = vec![];

    // Download crates multithreaded
    Pool::new(crates.download_threads as u32).scoped(|scoped| {
        diff.foreach(
            &mut |delta, _| {
                let df = delta.new_file();
                let p = df.path().unwrap();
                if p == Path::new("config.json") {
                    return true;
                }

                // Get the data for this crate file
                let oid = df.id();
                if oid.is_zero() {
                    // The crate was removed, continue to next crate
                    removed_crates.push(p.to_path_buf());
                    return true;
                }
                let blob = repo.find_blob(oid).unwrap();
                let data = blob.content();

                // Download one crate for each of the versions in the crate file
                for line in Cursor::new(data).lines() {
                    let line = line.unwrap();
                    let c: CrateEntry = serde_json::from_str(&line).unwrap();
                    let s = sender.clone();
                    scoped.execute(move || {
                        match sync_one_crate_entry(
                            path,
                            crates_source,
                            mirror.retries,
                            &c,
                            user_agent,
                        ) {
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
                                s.send(ProgressBarMessage::Println(format!(
                                    "Downloading {} {} failed: {:?}",
                                    &c.name, &c.vers, e
                                )))
                                .expect("Channel send should not fail");
                            }
                        }
                        s.send(ProgressBarMessage::Increment)
                            .expect("progress bar increment error");
                    });
                }

                true
            },
            None,
            None,
            None,
        )
        .unwrap();
    });

    // Delete any removed crates
    for rc in removed_crates {
        fs::remove_file(repo_path.join(rc))?;
    }

    sender
        .send(ProgressBarMessage::Done)
        .expect("Channel send should not fail");
    pb_thread.join().expect("Thread join should not fail");

    Ok(())
}
