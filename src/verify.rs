use std::{
    io::{BufRead, Cursor},
    path::Path,
    time::Duration,
};

use console::style;
use git2::Repository;
use indicatif::{ProgressBar, ProgressFinish, ProgressStyle};
use thiserror::Error;

use crate::{
    crates::{get_crate_path, CrateEntry},
    progress_bar::padded_prefix_message,
};

///
/// This variable is here to avoid to have false positive regarding crates.io [issue#1593](https://github.com/rust-lang/crates.io/issues/1593).
///
static CRATES_403: [(&str, &str); 22] = [
    ("glib-2-0-sys", "0.0.1"),
    ("glib-2-0-sys", "0.0.2"),
    ("glib-2-0-sys", "0.0.3"),
    ("glib-2-0-sys", "0.0.4"),
    ("glib-2-0-sys", "0.0.5"),
    ("glib-2-0-sys", "0.0.6"),
    ("glib-2-0-sys", "0.0.7"),
    ("glib-2-0-sys", "0.0.8"),
    ("glib-2-0-sys", "0.1.0"),
    ("glib-2-0-sys", "0.1.1"),
    ("glib-2-0-sys", "0.1.2"),
    ("glib-2-0-sys", "0.2.0"),
    ("gobject-2-0-sys", "0.0.2"),
    ("gobject-2-0-sys", "0.0.3"),
    ("gobject-2-0-sys", "0.0.2"),
    ("gobject-2-0-sys", "0.0.4"),
    ("gobject-2-0-sys", "0.0.5"),
    ("gobject-2-0-sys", "0.0.6"),
    ("gobject-2-0-sys", "0.0.7"),
    ("gobject-2-0-sys", "0.0.8"),
    ("gobject-2-0-sys", "0.1.0"),
    ("gobject-2-0-sys", "0.2.0"),
];

#[derive(Error, Debug)]
pub enum VerifyError {
    #[error("Git error: {0}")]
    GitError(#[from] git2::Error),

    #[error("Missing crate(s): {0:?}")]
    MissingCrates(Vec<CrateEntry>),
}

pub(crate) async fn verify_mirror(path: std::path::PathBuf) -> Result<(), VerifyError> {
    // Checking existence of local index
    let repo_path = path.join("crates.io-index");

    if !repo_path.join(".git").exists() {
        eprintln!("No index repository found in {}.", repo_path.display())
    }

    let prefix = padded_prefix_message(1, 1, "Comparing crates.io and mirror coherence");

    // Getting diff tree from local crates.io repository.
    let repo = Repository::open(repo_path)?;
    let master = repo.find_reference("refs/heads/master")?;
    let master_tree = master.peel_to_tree()?;
    let diff = repo.diff_tree_to_tree(None, Some(&master_tree), None)?;

    let mut missing_crates = Vec::new();

    let pb = ProgressBar::new_spinner()
        .with_style(
            ProgressStyle::default_bar()
                .template("{prefix} {wide_bar} {spinner} [{elapsed_precise}]")
                .expect("Something went wrong with the template.")
                .progress_chars("  "),
        )
        .with_prefix(prefix)
        .with_finish(ProgressFinish::AndLeave);
    pb.enable_steady_tick(Duration::from_millis(10));

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

            let oid = df.id();
            if oid.is_zero() {
                return true;
            }
            let blob = repo.find_blob(oid).unwrap();
            let data = blob.content();

            // Iterating over each line of a JSON file from local crates.io repository
            for line in Cursor::new(data).lines() {
                let line = line.unwrap();
                let crate_entry: CrateEntry = match serde_json::from_str(&line) {
                    Ok(c) => c,
                    Err(_) => {
                        continue;
                    }
                };

                // Building crates local path.
                let file_path =
                    get_crate_path(&path, crate_entry.get_name(), crate_entry.get_vers()).unwrap();

                // Checking if crate is missing.
                if !CRATES_403
                    .iter()
                    .any(|it| it.0 == crate_entry.get_name() && it.1 == crate_entry.get_vers())
                    && !crate_entry.yanked
                    && !file_path.exists()
                {
                    missing_crates.push(crate_entry);
                }
            }

            true
        },
        None,
        None,
        None,
    )?;

    if !missing_crates.is_empty() {
        return Err(VerifyError::MissingCrates(missing_crates));
    }

    eprintln!("{}", style("Verification successful.").bold());

    Ok(())
}
