use crate::{paths::Installation, runtime};
use anyhow::{Context, Result};
use std::path::PathBuf;

pub(crate) fn checkout(paths: &Installation, mut args: impl Iterator<Item = String>) -> Result<()> {
    let ref_name = args.next().context("missing ref")?;
    let dest = args.next().context("missing destination")?;
    runtime::checkout_ref(paths, &ref_name, PathBuf::from(dest))
}

pub(crate) fn inspect(paths: &Installation, refs: Vec<String>) -> Result<()> {
    runtime::inspect_refs(paths, &refs)
}
