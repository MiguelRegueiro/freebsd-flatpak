use crate::installation::{self as runtime, installation_paths::Installation};
use anyhow::{bail, Result};

pub(crate) fn cmd_prune(paths: &Installation, args: Vec<String>) -> Result<()> {
    if !args.is_empty() {
        bail!("usage: flatpak prune");
    }
    let (total, pruned, bytes) = runtime::prune_repo(paths)?;
    println!("Pruned {pruned} of {total} objects ({bytes} bytes reclaimed)");
    Ok(())
}
