use crate::mirror::{MirrorSection, CratesSection};
use console::style;

pub fn sync(mirror: &MirrorSection, crates: &CratesSection) {
    eprintln!("{}", style("Syncing Crates repositories...").bold());

    eprintln!("{} Syncing crates.io-index repository...", style("[1/3]").bold());

    eprintln!("{} Syncing crates...", style("[2/3]").bold());

    if let Some(base_url) = &mirror.base_url {
        eprintln!("{} Rewriting config.json URL in index...", style("[3/3]").bold());
    } else {

    }

    eprintln!("{}", style("Syncing Crates repositories complete!").bold());
}