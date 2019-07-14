use std::path::Path;
use std::{fs, io};

use serde_derive::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct MirrorSection {
    download_threads: usize,
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct RustupSection {
    sync: bool,
    verify_sha256: bool,
    source: String,
    keep_latest_stables: usize,
    keep_latest_betas: usize,
    keep_latest_nightlies: usize,
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct CratesSection {
    sync: bool,
    verify_sha256: bool,
    source: String,
    source_index: String,
    rewrite_url: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct Mirror {
    mirror: MirrorSection,
    rustup: RustupSection,
    crates: CratesSection,
}

pub(crate) fn create_mirror_directories(path: &Path) -> Result<(), io::Error> {
    fs::create_dir_all(path.join("crates"))?;
    fs::create_dir_all(path.join("rustup/dist"))?;
    fs::create_dir_all(path.join("dist"))?;
    fs::create_dir_all(path.join("crates.io-index"))?;
    Ok(())
}

pub(crate) fn create_mirror_toml(path: &Path) -> Result<bool, io::Error> {
    if path.join("mirror.toml").exists() {
        return Ok(false)
    }

    let mirror = Mirror {
        mirror: MirrorSection {
            download_threads: 4,
        },
        rustup: RustupSection {
            sync: true,
            verify_sha256: true,
            source: "https://static.rust-lang.org".to_string(),
            keep_latest_stables: 1,
            keep_latest_betas: 1,
            keep_latest_nightlies: 1,
        },
        crates: CratesSection {
            sync: true,
            verify_sha256: true,
            source: "https://crates.io/api/v1/crates".to_string(),
            source_index: "https://github.com/rust-lang/crates.io-index".to_string(),
            rewrite_url: "http://panamax.internal/crates".to_string(),
        },
    };
    let mirror_str = toml::to_string(&mirror).expect("Could not create TOML content");
    fs::write(path.join("mirror.toml"), mirror_str)?;
    Ok(true)
}

pub(crate) fn init(path: &Path) -> Result<(), io::Error> {
    create_mirror_directories(path)?;
    if create_mirror_toml(path)? {
        eprintln!("Successfully created mirror base at `{}`.", path.display());
    } else {
        eprintln!("Mirror base already exists at `{}`.", path.display());
    }
    eprintln!("Make any desired changes to {}/mirror.toml, then run panamax sync {}.", path.display(), path.display());

    Ok(())
}

pub(crate) fn sync(path: &Path) -> Result<(), io::Error> {
    Ok(())
}
