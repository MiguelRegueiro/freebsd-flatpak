use crate::{paths::Installation, state};
use anyhow::{bail, Result};

pub(crate) fn cmd_list(paths: &Installation, args: Vec<String>) -> Result<()> {
    if !args.is_empty() {
        bail!("usage: flatpak list");
    }
    let apps = state::list_apps(paths)?;
    if apps.is_empty() {
        println!("No installed apps");
        return Ok(());
    }
    println!(
        "{:<34} {:<8} {:<8} {:<32} Command",
        "Application ID", "Arch", "Branch", "Runtime"
    );
    for app in apps {
        println!(
            "{:<34} {:<8} {:<8} {:<32} {}",
            app.app_id, app.arch, app.branch, app.runtime_ref, app.command
        );
    }
    Ok(())
}
