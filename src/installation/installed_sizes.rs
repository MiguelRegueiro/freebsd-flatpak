use super::generation_cleanup::deployment_data;
use super::installation_paths::Installation;
use super::ExtensionRecord;
use anyhow::{Context, Result};
use std::fs;

pub fn list_extensions(paths: &Installation) -> Result<Vec<ExtensionRecord>> {
    let mut extensions = Vec::new();
    if !paths.extensions().is_dir() {
        return Ok(extensions);
    }
    for entry in fs::read_dir(paths.extensions()).context("read extension directory")? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(data) = deployment_data(&entry.path())? else {
            continue;
        };
        extensions.push(ExtensionRecord {
            origin: data.origin,
            ref_name: data.ref_name,
            commit: data.commit,
            installed_size: data.installed_size,
            checkout_dir: entry.path(),
        });
    }
    extensions.sort_by(|left, right| left.ref_name.cmp(&right.ref_name));
    Ok(extensions)
}

#[cfg(test)]
#[path = "tests/installed_sizes.rs"]
mod tests;
