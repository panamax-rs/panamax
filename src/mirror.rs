use std::path::Path;
use std::{fs, io};

use console::style;
use reqwest::header::HeaderValue;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::crates::is_new_crates_format;
use crate::crates_index::rewrite_config_json;

#[derive(Error, Debug)]
pub enum MirrorError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("TOML deserialization error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("Config file error: {0}")]
    Config(String),
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ConfigMirror {
    pub retries: usize,
    pub contact: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ConfigRustup {
    pub sync: bool,
    pub download_threads: usize,
    pub source: String,
    pub platforms_unix: Option<Vec<String>>,
    pub platforms_windows: Option<Vec<String>>,
    pub keep_latest_stables: Option<usize>,
    pub keep_latest_betas: Option<usize>,
    pub keep_latest_nightlies: Option<usize>,
    pub pinned_rust_versions: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ConfigCrates {
    pub sync: bool,
    pub download_threads: usize,
    pub source: String,
    pub source_index: String,
    pub use_new_crates_format: Option<bool>,
    pub base_url: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Config {
    pub mirror: ConfigMirror,
    pub rustup: Option<ConfigRustup>,
    pub crates: Option<ConfigCrates>,
}

pub fn create_mirror_directories(path: &Path) -> Result<(), io::Error> {
    // Rustup directories
    fs::create_dir_all(path.join("rustup/dist"))?;
    fs::create_dir_all(path.join("dist"))?;

    // Crates directories
    fs::create_dir_all(path.join("crates.io-index"))?;
    fs::create_dir_all(path.join("crates"))?;
    Ok(())
}

pub fn create_mirror_toml(path: &Path) -> Result<bool, io::Error> {
    if path.join("mirror.toml").exists() {
        return Ok(false);
    }

    let mirror = include_str!("mirror.default.toml");

    fs::write(path.join("mirror.toml"), mirror)?;

    Ok(true)
}

pub fn load_mirror_toml(path: &Path) -> Result<Config, MirrorError> {
    Ok(toml::from_str(&fs::read_to_string(
        path.join("mirror.toml"),
    )?)?)
}

pub fn init(path: &Path) -> Result<(), MirrorError> {
    create_mirror_directories(path)?;
    if create_mirror_toml(path)? {
        eprintln!("Successfully created mirror base at `{}`.", path.display());
    } else {
        eprintln!("Mirror base already exists at `{}`.", path.display());
    }
    eprintln!(
        "Make any desired changes to {}/mirror.toml, then run panamax sync {}.",
        path.display(),
        path.display()
    );

    Ok(())
}

pub fn default_user_agent() -> String {
    eprintln!("{}", style("No contact information was provided!").bold());
    eprintln!(
        "As per the crates.io crawling policy, lacking this may cause your IP to be blocked."
    );
    eprintln!("Please set this in your mirror.toml.");
    eprintln!();
    format!(
        "Panamax/{} (No contact information provided)",
        env!("CARGO_PKG_VERSION")
    )
}

pub fn sync(path: &Path) -> Result<(), MirrorError> {
    if !path.join("mirror.toml").exists() {
        eprintln!(
            "Mirror base not found! Run panamax init {} first.",
            path.display()
        );
        return Ok(());
    }
    let mirror = load_mirror_toml(path)?;

    // Fail if use_new_crates_format is not true, and old format is detected.
    // If use_new_crates_format is true and new format is detected, warn the user.
    // If use_new_crates_format is true, ignore the format and assume it's new.
    if let Some(crates) = &mirror.crates {
        if crates.sync {
            if crates.use_new_crates_format != Some(true) {
                if is_new_crates_format(&path.join("crates"))? {
                    eprintln!("Your crates/ directory is using the new 0.3 format, however");
                    eprintln!("use_new_crates_format is not set in mirror.toml. To remove this warning,");
                    eprintln!("Please add 'use_new_crates_format = true' to mirror.toml's [crates] section.");
                    eprintln!();
                } else {
                    eprintln!("Your crates directory is using the old 0.2 format, however");
                    eprintln!("Panamax 0.3 has deprecated this format for a new one.");
                    eprintln!("Please delete crates/ and crates.io-index/ from your mirror to continue,");
                    eprintln!("and add 'use_new_crates_format = true' to mirror.toml's [crates] section.");
                    return Ok(());
                }
            }
        }
    }
    
    // Handle the contact information

    let user_agent_str = if let Some(ref contact) = mirror.mirror.contact {
        if contact != "your@email.com" {
            format!("Panamax/{} ({})", env!("CARGO_PKG_VERSION"), contact)
        } else {
            default_user_agent()
        }
    } else {
        default_user_agent()
    };

    // Set the user agent with contact information.
    let user_agent = match HeaderValue::from_str(&user_agent_str) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Your contact information contains invalid characters!");
            eprintln!("It's recommended to use a URL or email address as contact information.");
            eprintln!("{:?}", e);
            return Ok(());
        }
    };

    if let Some(rustup) = mirror.rustup {
        if rustup.sync {
            crate::rustup::sync(path, &mirror.mirror, &rustup, &user_agent)?;
        } else {
            eprintln!("Rustup sync is disabled, skipping...");
        }
    } else {
        eprintln!("Rustup section missing, skipping...");
    }

    if let Some(crates) = mirror.crates {
        if crates.sync {
            sync_crates(path, &mirror.mirror, &crates, &user_agent);
        } else {
            eprintln!("Crates sync is disabled, skipping...");
        }
    } else {
        eprintln!("Crates section missing, skipping...");
    }

    eprintln!("Sync complete.");

    Ok(())
}

/// Rewrite the config.toml only.
///
/// Note that this will also fast-forward the repository
/// from origin/master, to keep a clean slate.
pub fn rewrite(path: &Path, base_url: Option<String>) -> Result<(), MirrorError> {
    if !path.join("mirror.toml").exists() {
        eprintln!(
            "Mirror base not found! Run panamax init {} first.",
            path.display()
        );
        return Ok(());
    }
    let mirror = load_mirror_toml(path)?;

    if let Some(crates) = mirror.crates {
        if let Some(base_url) = base_url.as_deref().or_else(|| crates.base_url.as_deref()) {
            if let Err(e) = rewrite_config_json(&path.join("crates.io-index"), base_url) {
                eprintln!("Updating crates.io-index config failed: {:?}", e);
            }
        } else {
            eprintln!("No base_url was provided.");
            eprintln!(
                "This needs to be provided by command line or in the mirror.toml to continue."
            )
        }
    } else {
        eprintln!("Crates section missing in mirror.toml.");
    }

    Ok(())
}

/// Synchronize and handle the crates.io-index repository.
pub fn sync_crates(
    path: &Path,
    mirror: &ConfigMirror,
    crates: &ConfigCrates,
    user_agent: &HeaderValue,
) {
    eprintln!("{}", style("Syncing Crates repositories...").bold());

    if let Err(e) = crate::crates_index::sync_crates_repo(path, crates) {
        eprintln!("Downloading crates.io-index repository failed: {:?}", e);
        eprintln!("You will need to sync again to finish this download.");
        return;
    }

    if let Err(e) = crate::crates::sync_crates_files(path, mirror, crates, user_agent) {
        eprintln!("Downloading crates failed: {:?}", e);
        eprintln!("You will need to sync again to finish this download.");
        return;
    }

    if let Err(e) = crate::crates_index::update_crates_config(path, crates) {
        eprintln!("Updating crates.io-index config failed: {:?}", e);
        eprintln!("You will need to sync again to finish this download.");
    }

    eprintln!("{}", style("Syncing Crates repositories complete!").bold());
}
