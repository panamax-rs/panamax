use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::{fs, io};

use console::style;
use reqwest::header::HeaderValue;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::crates::is_new_crates_format;
use crate::crates_index::rewrite_config_json;
use crate::download::download_string;
use crate::rustup::Channel;
use crate::serve::TlsConfig;

#[derive(Error, Debug)]
pub enum MirrorError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("TOML deserialization error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("Config file error: {0}")]
    Config(String),
    #[error("Command line error: {0}")]
    CmdLine(String),
    #[error("Download error: {0}")]
    DownloadError(#[from] crate::download::DownloadError),
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
    pub download_dev: Option<bool>,
    pub download_gz: Option<bool>,
    pub download_xz: Option<bool>,
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

pub async fn sync(path: &Path) -> Result<(), MirrorError> {
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
        if crates.sync && !is_new_crates_format(&path.join("crates"))? {
            eprintln!("Your crates directory is using the old 0.2 format, however");
            eprintln!("Panamax 0.3 has deprecated this format for a new one.");
            eprintln!("Please delete crates/ from your mirror directory to continue.");
            return Ok(());
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
            crate::rustup::sync(path, &mirror.mirror, &rustup, &user_agent).await?;
        } else {
            eprintln!("Rustup sync is disabled, skipping...");
        }
    } else {
        eprintln!("Rustup section missing, skipping...");
    }

    if let Some(crates) = mirror.crates {
        if crates.sync {
            sync_crates(path, &mirror.mirror, &crates, &user_agent).await;
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
pub async fn sync_crates(
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

    if let Err(e) = crate::crates::sync_crates_files(path, mirror, crates, user_agent).await {
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

pub fn serve(
    path: PathBuf,
    listen: Option<IpAddr>,
    port: Option<u16>,
    cert_path: Option<PathBuf>,
    key_path: Option<PathBuf>,
) -> Result<(), MirrorError> {
    let listen = listen.unwrap_or_else(|| {
        "::".parse()
            .expect(":: IPv6 address should never fail to parse")
    });
    let port = port.unwrap_or_else(|| if cert_path.is_some() { 8443 } else { 8080 });
    let socket_addr = SocketAddr::new(listen, port);

    let rt = tokio::runtime::Runtime::new()?;

    match (cert_path, key_path) {
        (Some(cert_path), Some(key_path)) => rt.block_on(crate::serve::serve(
            path,
            socket_addr,
            Some(TlsConfig {
                cert_path,
                key_path,
            }),
        )),
        (None, None) => rt.block_on(crate::serve::serve(path, socket_addr, None)),
        (Some(_), None) => {
            return Err(MirrorError::CmdLine(
                "cert_path set but key_path not set.".to_string(),
            ))
        }
        (None, Some(_)) => {
            return Err(MirrorError::CmdLine(
                "key_path set but cert_path not set.".to_string(),
            ))
        }
    };

    Ok(())
}

/// Print out a list of all platforms.
pub(crate) async fn list_platforms(source: String, channel: String) -> Result<(), MirrorError> {
    let channel_url = format!("{}/dist/channel-rust-{}.toml", source, channel);
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

    println!(
        "All currently available platforms for the {} channel:",
        channel
    );
    for t in targets {
        println!("  {}", t);
    }

    Ok(())
}
