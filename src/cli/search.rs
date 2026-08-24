use crate::installation::installation_paths::Installation;
use crate::remotes;
use anyhow::{bail, Result};

pub(crate) fn cmd_search(paths: &Installation, args: Vec<String>) -> Result<()> {
    if args.len() != 1 {
        bail!("usage: flatpak search <query>");
    }
    let results = remotes::search_apps(paths, &args[0])?;
    if results.is_empty() {
        println!("No matches");
        return Ok(());
    }
    println!(
        "{:<20} {:<42} {:<8} {:<12} Ref",
        "Remote", "Application ID", "Arch", "Branch"
    );
    for result in results.into_iter().take(50) {
        println!(
            "{:<20} {:<42} {:<8} {:<12} {}",
            result.remote, result.app_id, result.arch, result.branch, result.app_ref
        );
    }
    Ok(())
}
