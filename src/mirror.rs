use std::path::Path;
use std::{fs, io};

use serde_derive::{Deserialize, Serialize};

quick_error! {
    #[derive(Debug)]
    pub enum MirrorError {
        Io(err: io::Error) {
            from()
        }
        Parse(err: toml::de::Error) {
            from()
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct MirrorSection {
    pub download_threads: usize,
    pub base_url: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct RustupSection {
    pub sync: bool,
    pub source: String,
    pub keep_latest_stables: Option<usize>,
    pub keep_latest_betas: Option<usize>,
    pub keep_latest_nightlies: Option<usize>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CratesSection {
    pub sync: bool,
    pub source: String,
    pub source_index: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Mirror {
    pub mirror: MirrorSection,
    pub rustup: Option<RustupSection>,
    pub crates: Option<CratesSection>,
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

    let mirror = Mirror {
        mirror: MirrorSection {
            download_threads: 4,
            base_url: Some("http://panamax.internal".to_string()),
        },
        rustup: Some(RustupSection {
            sync: true,
            source: "https://static.rust-lang.org".to_string(),
            keep_latest_stables: Some(1),
            keep_latest_betas: Some(1),
            keep_latest_nightlies: Some(1),
        }),
        crates: Some(CratesSection {
            sync: true,
            source: "https://crates.io/api/v1/crates".to_string(),
            source_index: "https://github.com/rust-lang/crates.io-index".to_string(),
        }),
    };
    let mirror_str = toml::to_string(&mirror).expect("Could not create mirror.toml content");
    fs::write(path.join("mirror.toml"), mirror_str)?;
    Ok(true)
}

pub fn load_mirror_toml(path: &Path) -> Result<Mirror, MirrorError> {
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

pub fn sync(path: &Path) -> Result<(), MirrorError>{
    if !path.join("mirror.toml").exists() {
        eprintln!("Mirror base not found! Run panamax init {} first.", path.display());
        return Ok(());
    }
    let mirror = load_mirror_toml(path)?;

    if let Some(rustup) = mirror.rustup {
        if rustup.sync {
            crate::rustup::sync(path, &mirror.mirror, &rustup);
        } else {
            eprintln!("Rustup sync is disabled, skipping...");
        }
    } else {
        eprintln!("Rustup section missing, skipping...");
    }

    if let Some(crates) = mirror.crates {
        if crates.sync {
            crate::crates::sync(path,&mirror.mirror, &crates);
        } else {
            eprintln!("Crates sync is disabled, skipping...");
        }
    } else {
        eprintln!("Crates section missing, skipping...");
    }

    Ok(())
}
