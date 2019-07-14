use crate::mirror::{MirrorSection, CratesSection};
use console::style;

pub fn sync(mirror: &MirrorSection, crates: &CratesSection) {
    eprintln!("{}", style("Syncing Crates repositories...").bold());

    eprintln!("{} Syncing crates.io-index repository...", style("[1/2]").bold());

    eprintln!("{} Syncing crates...", style("[2/2]").bold());

    if let Some(base_url) = &mirror.base_url {
        eprintln!("      Rewriting URL in crates.io-index config.json...");
    }

    eprintln!("{}", style("Syncing Crates repositories complete!").bold());
}