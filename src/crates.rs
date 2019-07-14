use crate::mirror::{MirrorSection, CratesSection, MirrorError};
use console::style;
use std::path::Path;

pub fn sync(path: &Path, mirror: &MirrorSection, crates: &CratesSection) -> Result<(), MirrorError> {
    eprintln!("{}", style("Syncing Crates repositories...").bold());

    eprintln!("{} Syncing crates.io-index repository...", style("[1/3]").bold());

    eprintln!("{} Syncing crates...", style("[2/3]").bold());

    if let Some(base_url) = &mirror.base_url {
        eprintln!("{} Rewriting config.json URL in index...", style("[3/3]").bold());
    } else {

    }

    eprintln!("{}", style("Syncing Crates repositories complete!").bold());

    Ok(())
}