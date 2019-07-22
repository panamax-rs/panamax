use crate::download::DownloadError;
use crate::mirror::{CratesSection, MirrorError, MirrorSection};
use crate::progress_bar::{progress_bar, ProgressBarMessage};
use console::style;
use git2::{FetchOptions, RemoteCallbacks, Repository, RepositoryInitOptions};
use std::io;
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

pub fn sync_crates_repo(path: &Path, crates: &CratesSection) {
    let repo_path = path.join("crates.io-index");
    // Clone a bare repo

    let prefix = format!(
        "{} Syncing crates.io-index repository...",
        style("[1/3]").bold()
    );
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

    if !repo_path.join(".git").exists() {
        let mut init_opts = RepositoryInitOptions::new();
        init_opts.origin_url(&crates.source_index);
        let repo = Repository::init_opts(repo_path, &init_opts).unwrap();

        repo.find_remote("origin")
            .unwrap()
            .fetch(&["master"], Some(&mut fetch_options), None)
            .unwrap();

    } else {
        let repo = Repository::open(repo_path).unwrap();
        repo.find_remote("origin")
            .unwrap()
            .fetch(&["master"], Some(&mut fetch_options), None)
            .unwrap();
    }
    sender.send(ProgressBarMessage::Done).unwrap();
    pb_thread.join().unwrap();
    // Only checkout and merge master once crate files are downloaded
}

pub fn sync(
    path: &Path,
    mirror: &MirrorSection,
    crates: &CratesSection,
) -> Result<(), MirrorError> {
    eprintln!("{}", style("Syncing Crates repositories...").bold());

    sync_crates_repo(path, crates);

    eprintln!("{} Syncing crates...", style("[2/3]").bold());

    if let Some(_base_url) = &mirror.base_url {
        eprintln!("{} Merging crates.io-index...", style("[3/3]").bold());
    } else {

    }

    eprintln!("{}", style("Syncing Crates repositories complete!").bold());

    Ok(())
}
