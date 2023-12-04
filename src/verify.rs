use std::{
    cmp::Ordering,
    convert::Infallible,
    io::{BufRead, Cursor, Write},
    ops::RangeInclusive,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use console::style;
use futures::StreamExt;
use git2::Repository;
use indicatif::{ProgressBar, ProgressFinish, ProgressStyle};
use reqwest::Client;
use warp::http::HeaderValue;

use crate::{
    crates::{
        cargo_lock_to_mirror_entries, get_crate_path, sync_one_crate_entry,
        vendor_path_to_mirror_entries, CrateEntry,
    },
    download::DownloadError,
    mirror::{default_user_agent, ConfigCrates, ConfigMirror, MirrorError},
    progress_bar::padded_prefix_message,
};

///
/// This variable is here to avoid to have false positive regarding crates.io [issue#1593](https://github.com/rust-lang/crates.io/issues/1593).
///
static CRATES_403: [(&str, &str); 23] = [
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
    ("gobject-2-0-sys", "0.0.9"),
    ("gobject-2-0-sys", "0.1.0"),
    ("gobject-2-0-sys", "0.2.0"),
];

/// Type used to represent user's input which will be used to indexed a `Vec`
#[derive(Debug, PartialEq, Eq)]
enum Input {
    Range(RangeInclusive<usize>),
    Vec(Vec<usize>),
    Usize(usize),
    Ignore,
}

impl Input {
    // Check if value is safe to useas an index for a given `Vec`'s length
    fn check(&self, length: usize) -> bool {
        match self {
            Input::Range(range) => *range.end() < length,
            Input::Usize(u) => *u < length,
            Input::Vec(v) => v.iter().all(|u| *u < length),
            Input::Ignore => false,
        }
    }
}

impl FromStr for Input {
    // `Infaillible` because if we can not parse input, `Self::Ignore` will be returned
    type Err = Infallible;

    /// Directly handling user input.
    /// All `0`s are ignored so that remove one in each cases can be safely done.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // If s is empty or if s contains spaces and dashes, ignore it
        if s.is_empty() || (s.contains(' ') && s.contains('-')) {
            Ok(Self::Ignore)
        } else if s.contains(' ') {
            // Parsing as a `Vec`, ignoring `0`
            let mut result: Vec<usize> = s
                .split(' ')
                .filter_map(|s| match s.parse() {
                    Ok(u) if u != 0 => Some(u),
                    _ => None,
                })
                .collect();
            if result.len() == 1 {
                // If only one element, return it as a `usize` minus one
                Ok(Self::Usize(result[0] - 1))
            } else if !result.is_empty() {
                // Sorting the `Vec` and remove one at each `usize`
                result.sort_unstable();
                result.iter_mut().for_each(|u| *u -= 1);
                Ok(Self::Vec(result))
            } else {
                Ok(Self::Ignore)
            }
        } else if s.contains('-') {
            // Parsing as a `Vec`, ignoring `0`
            let bounds: Vec<usize> = s
                .split('-')
                .filter_map(|s| match s.parse::<usize>() {
                    Ok(u) if u != 0 => Some(u),
                    _ => None,
                })
                .collect();
            if bounds.len() == 2 {
                // If we have exactly two elements
                let start = bounds[0] - 1;
                let end = bounds[1] - 1;
                match start.cmp(&end) {
                    // x < y => x..=y
                    Ordering::Less => Ok(Self::Range(RangeInclusive::new(start, end))),
                    // x == y => x
                    Ordering::Equal => Ok(Self::Usize(start)),
                    Ordering::Greater => Ok(Self::Ignore),
                }
            } else {
                Ok(Self::Ignore)
            }
        } else {
            // Trying to parse it as a single `usize` different from `0`, otherwise we ignore it
            s.parse::<usize>().map_or(Ok(Self::Ignore), |u| {
                if u == 0 {
                    Ok(Self::Ignore)
                } else {
                    // Removing one
                    Ok(Self::Usize(u - 1))
                }
            })
        }
    }
}

pub(crate) async fn verify_mirror(
    path: std::path::PathBuf,
    current_step: &mut usize,
    steps: usize,
    vendor_path: Option<PathBuf>,
    cargo_lock_filepath: Option<PathBuf>,
) -> Result<Option<Vec<CrateEntry>>, MirrorError> {
    // Checking existence of local index
    let repo_path = path.join("crates.io-index");

    if !repo_path.join(".git").exists() {
        eprintln!("No index repository found in {}.", repo_path.display())
    }

    let prefix = padded_prefix_message(
        *current_step,
        steps,
        "Comparing local crates.io and mirror coherence",
    );

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

    // Getting diff tree from local crates.io repository.
    let repo = Repository::open(repo_path)?;
    let master = repo.find_reference("refs/heads/master")?;
    let master_tree = master.peel_to_tree()?;
    let diff = repo.diff_tree_to_tree(None, Some(&master_tree), None)?;

    let mut missing_crates = Vec::new();

    let is_crate_whitelist_only = vendor_path.is_some() || cargo_lock_filepath.is_some();
    // if a vendor_path, parse the filepath for Cargo.toml files for each crate, filling vendors
    let mut mirror_entries = vec![];
    vendor_path_to_mirror_entries(&mut mirror_entries, vendor_path.as_ref());
    cargo_lock_to_mirror_entries(&mut mirror_entries, cargo_lock_filepath.as_ref());

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

                // Checking only whitelisted crates if supplied
                if is_crate_whitelist_only
                    && !mirror_entries.iter().any(|it| {
                        it.get_name() == crate_entry.get_name()
                            && it.get_vers() == crate_entry.get_vers()
                    })
                {
                    continue;
                }

                // Building crates local path.
                let file_path =
                    get_crate_path(&path, crate_entry.get_name(), crate_entry.get_vers()).unwrap();

                // Checking if crate is missing.
                if !CRATES_403
                    .iter()
                    .any(|it| it.0 == crate_entry.get_name() && it.1 == crate_entry.get_vers())
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

    pb.finish();
    *current_step += 1;

    if !missing_crates.is_empty() {
        return Ok(Some(missing_crates));
    }

    eprintln!("{}", style("Verification successful.").bold());

    Ok(None)
}

/// This method is giving choice to users whether to filter some crates or not before downloading.
pub(crate) async fn handle_user_input(
    mut missing_crates: Vec<CrateEntry>,
) -> Result<Vec<CrateEntry>, MirrorError> {
    println!("Found {} missing crates:", missing_crates.len());
    missing_crates.iter().enumerate().for_each(|(i, c)| {
        println!(
            "   {}: {} - version {}",
            // Adding one to index here to start presenting to users from `1..=missing_crates.len()`
            style((i + 1).to_string()).bold(),
            c.get_name(),
            c.get_vers()
        );
    });
    println!("{}",
        style("Missing crates to download (e.g.: '1 2 3' or  '1-3') [Leave empty for downloading all of them]:").bold()
    );
    std::io::stdout().flush()?;
    let mut input = String::new();
    match std::io::stdin().read_line(&mut input)? {
        // Handling EOF
        0 => Ok(missing_crates),
        _ => {
            // Popping '\n'
            input.pop();
            // Safe to unwrap here
            let input = input.parse::<Input>().unwrap();
            if input.check(missing_crates.len()) {
                // Input is not respecting `Vec` bounds, ignoring request
                Ok(Vec::new())
            } else {
                match input {
                    Input::Ignore => Ok(missing_crates),
                    Input::Range(range) => {
                        range.into_iter().for_each(|u| {
                            missing_crates.remove(u);
                        });
                        Ok(missing_crates)
                    }
                    Input::Usize(u) => Ok(vec![missing_crates.remove(u)]),
                    Input::Vec(v) => {
                        v.into_iter().for_each(|u| {
                            missing_crates.remove(u);
                        });
                        Ok(missing_crates)
                    }
                }
            }
        }
    }
}

/// This method is cactually fixing mirror by downloading missing crates.
pub(crate) async fn fix_mirror(
    mirror_config: &ConfigMirror,
    crates_config: &ConfigCrates,
    path: PathBuf,
    crates_to_fetch: Vec<CrateEntry>,
    current_step: &mut usize,
    steps: usize,
) -> Result<(), MirrorError> {
    let prefix = padded_prefix_message(*current_step, steps, "Repairing mirror");

    let pb = ProgressBar::new(crates_to_fetch.len() as u64)
        .with_style(
            ProgressStyle::default_bar()
                .template(
                    "{prefix} {wide_bar} {pos}/{len} [{elapsed_precise} / {duration_precise}]",
                )
                .expect("Something went wrong with the template.")
                .progress_chars("█▉▊▋▌▍▎▏  "),
        )
        .with_prefix(prefix)
        .with_finish(ProgressFinish::AndLeave);
    pb.enable_steady_tick(Duration::from_millis(10));

    // Getting crates' source from config
    let crates_source = if crates_config.source != "https://crates.io/api/v1/crates" {
        Some(crates_config.source.as_str())
    } else {
        None
    };

    // Handle the contact information
    let user_agent_str =
        mirror_config
            .contact
            .as_ref()
            .map_or_else(default_user_agent, |contact| {
                if contact != "your@email.com" {
                    format!("Panamax/{} ({})", env!("CARGO_PKG_VERSION"), contact)
                } else {
                    default_user_agent()
                }
            });

    // Set the user agent with contact information.
    let user_agent = match HeaderValue::from_str(&user_agent_str) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Your contact information contains invalid characters!");
            eprintln!("It's recommended to use a URL or email address as contact information.");
            eprintln!("{e:?}");
            return Ok(());
        }
    };

    let client = Client::new();

    // This code is copied from `crates::sync_crates_files` and could be mutualised in a future commit.
    // For example in a function within module crates (e.g. `crates::build_and_run_tasks`)
    let tasks = futures::stream::iter(crates_to_fetch.into_iter())
        .map(|c| {
            // Duplicate variables used in the async closure.
            let client = client.clone();
            let path = path.clone();
            let mirror_retries = mirror_config.retries;
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
        .buffer_unordered(crates_config.download_threads)
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

    pb.finish_and_clear();
    *current_step += 1;
    Ok(())
}

#[cfg(test)]
mod test {

    mod input {
        use crate::verify::Input;

        #[test]
        fn true_range() {
            let input = "1-5".to_string();
            let expected_result = Input::Range(0usize..=4);
            let result = input.parse::<Input>().unwrap();
            assert_eq!(expected_result, result);
        }

        #[test]
        fn false_range_true_usize() {
            let input = "1-1".to_string();
            let expected_result = Input::Usize(0);
            let result = input.parse::<Input>().unwrap();
            assert_eq!(expected_result, result);
        }

        #[test]
        fn garbage_range_1() {
            let input = "foo-bar".to_string();
            let expected_result = Input::Ignore;
            let result = input.parse::<Input>().unwrap();
            assert_eq!(expected_result, result);
        }

        #[test]
        fn garbage_range_2() {
            let input = "1-5 7".to_string();
            let expected_result = Input::Ignore;
            let result = input.parse::<Input>().unwrap();
            assert_eq!(expected_result, result);
        }

        #[test]
        fn garbage_range_3() {
            let input = "1-foo".to_string();
            let expected_result = Input::Ignore;
            let result = input.parse::<Input>().unwrap();
            assert_eq!(expected_result, result);
        }

        #[test]
        fn garbage_range_4() {
            let input = "5-1".to_string();
            let expected_result = Input::Ignore;
            let result = input.parse::<Input>().unwrap();
            assert_eq!(expected_result, result);
        }

        #[test]
        fn garbage_range_5() {
            let input = "0-2".to_string();
            let expected_result = Input::Ignore;
            let result = input.parse::<Input>().unwrap();
            assert_eq!(expected_result, result);
        }

        #[test]
        fn garbage_range_6() {
            let input = "0-0".to_string();
            let expected_result = Input::Ignore;
            let result = input.parse::<Input>().unwrap();
            assert_eq!(expected_result, result);
        }

        #[test]
        fn true_vec() {
            let input = "1 2 5 9".to_string();
            let expected_result = Input::Vec(vec![0, 1, 4, 8]);
            let result = input.parse::<Input>().unwrap();
            assert_eq!(expected_result, result);
        }

        #[test]
        fn true_vec_shuffled() {
            let input = "6 4 8 2".to_string();
            let expected_result = Input::Vec(vec![1, 3, 5, 7]);
            let result = input.parse::<Input>().unwrap();
            assert_eq!(expected_result, result);
        }

        #[test]
        fn garbage_vec() {
            let input = "foo bar fubar".to_string();
            let expected_result = Input::Ignore;
            let result = input.parse::<Input>().unwrap();
            assert_eq!(expected_result, result);
        }

        #[test]
        fn some_garbage_vec_1() {
            let input = "1 bar 6".to_string();
            let expected_result = Input::Vec(vec![0, 5]);
            let result = input.parse::<Input>().unwrap();
            assert_eq!(expected_result, result);
        }

        #[test]
        fn some_garbage_vec_2() {
            let input = "0 2 6".to_string();
            let expected_result = Input::Vec(vec![1, 5]);
            let result = input.parse::<Input>().unwrap();
            assert_eq!(expected_result, result);
        }

        #[test]
        fn some_garbage_vec_3() {
            let input = "4 0 2".to_string();
            let expected_result = Input::Vec(vec![1, 3]);
            let result = input.parse::<Input>().unwrap();
            assert_eq!(expected_result, result);
        }

        #[test]
        fn true_usize() {
            let input = "42".to_string();
            let expected_result = Input::Usize(41);
            let result = input.parse::<Input>().unwrap();
            assert_eq!(expected_result, result);
        }

        #[test]
        fn garbage_usize_1() {
            let input = "foo".to_string();
            let expected_result = Input::Ignore;
            let result = input.parse::<Input>().unwrap();
            assert_eq!(expected_result, result);
        }

        #[test]
        fn garbage_usize_2() {
            let input = "0".to_string();
            let expected_result = Input::Ignore;
            let result = input.parse::<Input>().unwrap();
            assert_eq!(expected_result, result);
        }

        #[test]
        fn full_garbage() {
            let input = "1-3 42".to_string();
            let expected_result = Input::Ignore;
            let result = input.parse::<Input>().unwrap();
            assert_eq!(expected_result, result);
        }
    }
}
