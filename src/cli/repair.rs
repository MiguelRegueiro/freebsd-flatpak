use crate::installation::{self as runtime, installation_paths::Installation};
use anyhow::{bail, Result};

pub(crate) fn cmd_repair(paths: &Installation, args: Vec<String>) -> Result<()> {
    if !args.is_empty() {
        bail!("usage: flatpak repair");
    }
    println!("Checking OSTree object integrity...");
    let checked = runtime::repair_repo(paths)?;
    println!("Checked {checked} objects; no corruption found");
    Ok(())
}
