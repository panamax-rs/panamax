use crate::download::{download, DownloadError};
use crate::mirror::{CratesSection, MirrorError, MirrorSection};
use crate::progress_bar::{progress_bar, ProgressBarMessage};
use console::style;
use git2::{
    FetchOptions, IndexEntry, IndexTime, Oid, Reference, RemoteCallbacks, Repository,
    RepositoryInitOptions, Signature, Tree,
};
use reqwest::header::HeaderValue;
use scoped_threadpool::Pool;
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Cursor};
use std::path::Path;

static DEFAULT_CONFIG_JSON_CONTENT: &'static [u8] = br#"{
  "dl": "https://crates.io/api/v1/crates",
  "api": "https://crates.io"
}
"#;

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
        SerializeError(err: serde_json::Error) {
            from()
        }
        GitError(err: git2::Error) {
            from()
        }
        GitTargetNotFound {}
    }
}

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

/// Sync the crates.io-index repository.
pub fn sync_crates_repo(path: &Path, crates: &CratesSection) -> Result<(), SyncError> {
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
            .expect("Channel send should not fail");
        true
    });
    let mut fetch_options = FetchOptions::new();
    fetch_options.remote_callbacks(remote_callbacks);

    let repo = if !repo_path.join(".git").exists() {
        let mut init_opts = RepositoryInitOptions::new();
        init_opts.origin_url(&crates.source_index);
        Repository::init_opts(repo_path, &init_opts)?
    } else {
        Repository::open(repo_path)?
    };
    repo.find_remote("origin")?
        .fetch(&["master"], Some(&mut fetch_options), None)?;
    sender
        .send(ProgressBarMessage::Done)
        .expect("Channel send should not fail");
    pb_thread.join().expect("Thread join should not fail");

    Ok(())
}

/// Synchronize the crate files themselves, using the index for a list of files.
// TODO: There are still many unwraps in the foreach sections. This needs to be fixed.
pub fn sync_crates_files(
    path: &Path,
    mirror: &MirrorSection,
    crates: &CratesSection,
    user_agent: &HeaderValue,
) -> Result<(), SyncError> {
    let prefix = format!("{} Syncing crates files...     ", style("[2/3]").bold());

    // For now, assume successful crates.io-index download
    let repo_path = path.join("crates.io-index");
    let repo = Repository::open(repo_path)?;

    // Set the crates.io URL, or None if default
    let crates_source = if crates.source == "https://crates.io/api/v1/crates" {
        None
    } else {
        Some(crates.source.as_ref())
    };

    // Find References for origin/master and master (if it exists)
    let origin_master = repo.find_reference("refs/remotes/origin/master")?;
    let master = repo.find_reference("refs/heads/master").ok();

    // Diff between the two references, or find all files if master doesn't exist
    let origin_tree = origin_master.peel_to_tree()?;
    let diff = if let Some(master) = master {
        let master_tree = master.peel_to_tree()?;
        repo.diff_tree_to_tree(Some(&master_tree), Some(&origin_tree), None)
    } else {
        repo.diff_tree_to_tree(None, Some(&origin_tree), None)
    }?;

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
    )?;

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
                                .expect("Channel send should not fail");
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

    sender
        .send(ProgressBarMessage::Done)
        .expect("Channel send should not fail");
    pb_thread.join().expect("Thread join should not fail");

    Ok(())
}

/// Check if the config.json in master matches what we're expecting.
/// If config.json doesn't match what we've set previously, we need to update it.
pub fn is_config_json_up_to_date(
    repo: &Repository,
    master_tree: &Tree,
    content: &[u8],
) -> Result<bool, SyncError> {
    let existing_config_json = master_tree.get_name("config.json").unwrap();

    let config_blob = existing_config_json.to_object(&repo)?.peel_to_blob()?;
    let config_blob_content = config_blob.content();

    Ok(config_blob_content == content)
}

/// Create a new config.json, add it to an index, and commit the index.
pub fn commit_new_config_json(
    repo: &Repository,
    master: &Reference,
    origin_master_tree: &Tree,
    signature: &Signature,
    content: &[u8],
) -> Result<(), SyncError> {
    // Get the git index and clear it.
    let mut index = repo.index()?;
    index.clear()?;

    // Read the origin master's files into the index.
    index.read_tree(&origin_master_tree)?;

    // Add our config.json change into the index.
    index.add_frombuffer(
        &IndexEntry {
            ctime: IndexTime::new(0, 0),
            mtime: IndexTime::new(0, 0),
            dev: 0,
            ino: 0,
            mode: 0o100644,
            uid: 0,
            gid: 0,
            file_size: 0,
            id: Oid::from_bytes(&[0; 20]).expect("OID from zeroes should not fail"),
            flags: 0,
            flags_extended: 0,
            path: b"config.json".to_vec(),
        },
        content,
    )?;

    // Write the index file back to disk (the .git/index file).
    // This ensures the git working directory doesn't get staged,
    // so the entire repository doesn't get deleted in master.
    let oid = index.write_tree()?;
    index.write()?;

    // Perform the actual commit
    let parent_commit = master.peel_to_commit()?;
    let tree = repo.find_tree(oid)?;

    repo.commit(
        Some("refs/heads/master"),
        &signature,
        &signature,
        "Update config.json to mirror URL",
        &tree,
        &[&parent_commit],
    )?;

    Ok(())
}

/// Create master by branching from origin/master.
pub fn create_master_branch(repo: &Repository, origin_master: &Reference) -> Result<(), SyncError> {
    let origin_commit = origin_master.peel_to_commit()?;
    if let Some(origin_target) = origin_master.target() {
        let b = repo.branch("master", &origin_commit, true)?;
        b.into_reference().set_target(origin_target, "")?;
    } else {
        Err(SyncError::GitTargetNotFound)?;
    }

    Ok(())
}

/// Merge new commits from origin/master into master.
pub fn merge_into_master(
    repo: &Repository,
    origin_master: &Reference,
    master: &Reference,
    signature: &Signature,
) -> Result<(), SyncError> {
    let origin_commit = origin_master.peel_to_commit()?;
    let origin_tree = origin_master.peel_to_tree()?;

    let master_commit = master.peel_to_commit()?;
    let master_tree = master.peel_to_tree()?;

    let merge_base = repo.merge_base(origin_commit.id(), master_commit.id())?;
    let ancestor_commit = repo.find_commit(merge_base)?;
    let ancestor = ancestor_commit.tree()?;

    let mut idx = repo.merge_trees(&ancestor, &master_tree, &origin_tree, None)?;

    let result_tree = repo.find_tree(idx.write_tree_to(repo)?)?;

    let _merge_commit = repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        "Merge origin/master into master",
        &result_tree,
        &[&master_commit, &origin_commit],
    )?;

    Ok(())
}

#[derive(Debug, Serialize)]
struct ConfigJson {
    dl: String,
    api: String,
}

/// Build the config.json content, based on what base_url is set to in mirror.toml.
pub fn build_config_json_content(base_url: &str) -> Result<Vec<u8>, SyncError> {
    let config_json = ConfigJson {
        dl: base_url.to_string(),
        api: base_url.to_string(),
    };

    Ok(serde_json::to_vec_pretty(&config_json)?)
}

/// Merge the crates.io-index's master branch with origin/master,
/// keeping config.json up to date.
pub fn merge_crates_repo(path: &Path, crates: &CratesSection) -> Result<(), SyncError> {
    eprintln!("{} Merging crates.io-index...  ", style("[3/3]").bold());

    let repo_path = path.join("crates.io-index");
    let repo = Repository::open(repo_path)?;

    let signature = Signature::now("Panamax", "panamax@panamax")?;

    let origin_master = repo.find_reference("refs/remotes/origin/master")?;
    let origin_master_tree = origin_master.peel_to_tree()?;

    if let Ok(master) = repo.find_reference("refs/heads/master") {
        // Attempt to merge origin/master into master.
        merge_into_master(&repo, &origin_master, &master, &signature)?;
    } else {
        // If master doesn't exist, branch from origin/master.
        create_master_branch(&repo, &origin_master)?;
    }

    // At this point, master should exist and be in a merged/consistent state.
    let master = repo.find_reference("refs/heads/master")?;
    let master_tree = master.peel_to_tree()?;

    // If base_url is set, update config.json if it needs to be updated.
    if let Some(ref base_url) = crates.base_url {
        let content = build_config_json_content(&base_url)?;
        if !is_config_json_up_to_date(&repo, &master_tree, &content)? {
            commit_new_config_json(&repo, &master, &origin_master_tree, &signature, &content)?;
        }
    } else {
        // This section is useful in case the user removes base_url after the fact.
        if !is_config_json_up_to_date(&repo, &master_tree, DEFAULT_CONFIG_JSON_CONTENT)? {
            commit_new_config_json(&repo, &master, &origin_master_tree, &signature, DEFAULT_CONFIG_JSON_CONTENT)?;
        }
    }

    Ok(())
}

/// Synchronize crates.io mirror.
pub fn sync(
    path: &Path,
    mirror: &MirrorSection,
    crates: &CratesSection,
    user_agent: &HeaderValue,
) -> Result<(), MirrorError> {
    eprintln!("{}", style("Syncing Crates repositories...").bold());

    if let Err(e) = sync_crates_repo(path, crates) {
        eprintln!("Downloading crates.io-index repository failed: {:?}", e);
        eprintln!("You will need to sync again to finish this download.");
        return Ok(());
    }

    if let Err(e) = sync_crates_files(path, mirror, crates, user_agent) {
        eprintln!("Downloading crates failed: {:?}", e);
        eprintln!("You will need to sync again to finish this download.");
        return Ok(());
    }

    if let Err(e) = merge_crates_repo(path, crates) {
        eprintln!("Merging crates.io-index repository failed: {:?}", e);
        eprintln!("You will need to sync again to finish this download.");
    }

    eprintln!("{}", style("Syncing Crates repositories complete!").bold());

    Ok(())
}
