use crate::download::{download, DownloadError};
use crate::mirror::{CratesSection, MirrorError, MirrorSection};
use crate::progress_bar::{progress_bar, ProgressBarMessage};
use console::style;
use git2::{FetchOptions, RemoteCallbacks, Repository, RepositoryInitOptions};
use reqwest::header::HeaderValue;
use scoped_threadpool::Pool;
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Cursor};
use std::path::Path;

quick_error! {
    #[derive(Debug)]
    pub enum SyncError {
        Io(err: io::Error) {
            from()
        }
        Download(err: DownloadError) {
            from()
        }
        FailedDownloads(count: usize) {}
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CrateEntry {
    name: String,
    vers: String,
    cksum: String,
    yanked: bool,
}

pub fn sync_one_crate_entry(
    path: &Path,
    source: Option<&str>,
    retries: usize,
    crate_entry: &CrateEntry,
    user_agent: &HeaderValue,
) -> Result<(), DownloadError> {
    // What's the URL, what's the download path

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

// Sync the crates.io-index repository
pub fn sync_crates_repo(path: &Path, crates: &CratesSection) {
    let repo_path = path.join("crates.io-index");

    let prefix = format!("{} Syncing crates.io-index...  ", style("[1/3]").bold());
    let (pb_thread, sender) = progress_bar(None, prefix);
    let mut remote_callbacks = RemoteCallbacks::new();
    remote_callbacks.transfer_progress(|p| {
        &sender
            .send(ProgressBarMessage::SetProgress(
                p.indexed_objects(),
                p.total_objects(),
            ))
            .unwrap();
        true
    });
    let mut fetch_options = FetchOptions::new();
    fetch_options.remote_callbacks(remote_callbacks);

    let repo = if !repo_path.join(".git").exists() {
        let mut init_opts = RepositoryInitOptions::new();
        init_opts.origin_url(&crates.source_index);
        Repository::init_opts(repo_path, &init_opts).unwrap()
    } else {
        Repository::open(repo_path).unwrap()
    };
    repo.find_remote("origin")
        .unwrap()
        .fetch(&["master"], Some(&mut fetch_options), None)
        .unwrap();
    sender.send(ProgressBarMessage::Done).unwrap();
    pb_thread.join().unwrap();
}

pub fn sync_crates_files(
    path: &Path,
    mirror: &MirrorSection,
    crates: &CratesSection,
    user_agent: &HeaderValue,
) {
    let prefix = format!("{} Syncing crates files...     ", style("[2/3]").bold());

    // For now, assume successful crates.io-index download
    let repo_path = path.join("crates.io-index");
    let repo = Repository::open(repo_path).unwrap();

    // Set the crates.io URL, or None if default
    let crates_source = if crates.source == "https://crates.io/api/v1/crates" {
        None
    } else {
        Some(crates.source.as_ref())
    };

    // Find References for origin/master and master (if it exists)
    let origin_master = repo.find_reference("refs/remotes/origin/master").unwrap();
    let master = repo.find_reference("refs/heads/master").ok();

    // Diff between the two references, or find all files if master doesn't exist
    let origin_tree = origin_master.peel_to_tree().unwrap();
    let diff = if let Some(master) = master {
        let master_tree = master.peel_to_tree().unwrap();
        repo.diff_tree_to_tree(Some(&master_tree), Some(&origin_tree), None)
    } else {
        repo.diff_tree_to_tree(None, Some(&origin_tree), None)
    }
    .unwrap();

    // Run one pass to figure out a total count
    let mut count = 0;
    diff.foreach(
        &mut |delta, _| {
            let df = delta.new_file();
            let p = df.path().unwrap();
            if p == Path::new("config.json") {
                return true;
            }
            let oid = df.id();
            let blob = repo.find_blob(oid).unwrap();
            let data = blob.content();
            count += Cursor::new(data).lines().count();
            true
        },
        None,
        None,
        None,
    )
    .unwrap();

    let (pb_thread, sender) = progress_bar(Some(count), prefix);

    Pool::new(crates.download_threads as u32).scoped(|scoped| {
        diff.foreach(
            &mut |delta, _| {
                let df = delta.new_file();
                let p = df.path().unwrap();
                if p == Path::new("config.json") {
                    return true;
                }
                let oid = df.id();

                let blob = repo.find_blob(oid).unwrap();
                let data = blob.content();
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
                            | Err(DownloadError::NotFound(_, _, _))
                            | Err(DownloadError::MismatchedHash(_, _)) => {}
                            Err(e) => {
                                s.send(ProgressBarMessage::Println(format!(
                                    "Downloading {} {} failed: {:?}",
                                    &c.name, &c.vers, e
                                )))
                                .unwrap();
                            }
                        }
                        &s.send(ProgressBarMessage::Increment);
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

    pb_thread.join().unwrap();
}

pub fn sync(
    path: &Path,
    mirror: &MirrorSection,
    crates: &CratesSection,
    user_agent: &HeaderValue,
) -> Result<(), MirrorError> {
    eprintln!("{}", style("Syncing Crates repositories...").bold());

    sync_crates_repo(path, crates);

    sync_crates_files(path, mirror, crates, user_agent);

    if let Some(_base_url) = &mirror.base_url {
        eprintln!("{} Merging crates.io-index...  ", style("[3/3]").bold());
    } else {

    }

    eprintln!("{}", style("Syncing Crates repositories complete!").bold());

    Ok(())
}
