mod desktop;
mod linuxulator;
mod runtime;
mod sandbox;

use anyhow::{Context, Result};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("checkout") => {
            let ref_name = args.next().context("missing ref")?;
            let dest = args.next().context("missing destination")?;
            runtime::checkout_ref(&ref_name, PathBuf::from(dest))
        }
        Some("inspect") => runtime::inspect_refs(),
        Some(cmd) => anyhow::bail!("unknown command: {cmd}"),
        None => {
            eprintln!("usage:");
            eprintln!("  freebsd-flatpak-poc inspect");
            eprintln!("  freebsd-flatpak-poc checkout <ostree-ref> <destination>");
            Ok(())
        }
    }
}
