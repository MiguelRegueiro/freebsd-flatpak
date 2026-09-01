use crate::flatpak_metadata::value;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;

pub(super) fn validate_extension_checkout(ref_name: &str, checkout_dir: &Path) -> Result<()> {
    let validation = || -> Result<()> {
        if !checkout_dir.is_dir() {
            bail!("checkout directory does not exist")
        }
        let marker_path = checkout_dir.join(".ostree-commit");
        let marker = fs::read_to_string(&marker_path)
            .with_context(|| format!("read deployment marker {}", marker_path.display()))?;
        let mut lines = marker.lines();
        let installed_ref = lines.next().context("deployment marker missing ref")?;
        if installed_ref != ref_name {
            bail!("deployment marker contains ref {installed_ref}, expected {ref_name}")
        }
        if lines.next().filter(|commit| !commit.is_empty()).is_none() {
            bail!("deployment marker missing commit")
        }
        lines
            .next()
            .context("deployment marker missing installed size")?
            .parse::<u64>()
            .context("deployment marker has invalid installed size")?;
        if lines.next().filter(|origin| !origin.is_empty()).is_none() {
            bail!("deployment marker missing origin")
        }
        let metadata_path = checkout_dir.join("metadata");
        let metadata = fs::read_to_string(&metadata_path)
            .with_context(|| format!("read extension metadata {}", metadata_path.display()))?;
        let expected_name = ref_name
            .strip_prefix("runtime/")
            .and_then(|value| value.split('/').next())
            .context("extension ref is not a runtime ref")?;
        let installed_name = value(&metadata, "Runtime", "name")
            .context("extension metadata is missing Runtime/name")?;
        if installed_name != expected_name {
            bail!("extension metadata contains name {installed_name}, expected {expected_name}")
        }
        let files_path = checkout_dir.join("files");
        if !files_path.is_dir() {
            bail!("files directory is missing: {}", files_path.display())
        }
        Ok(())
    };

    validation().with_context(|| {
        format!(
            "required extension {ref_name} is missing or corrupt at {}; run `flatpak update` or `flatpak repair`",
            checkout_dir.display()
        )
    })
}

pub fn runtime_checkout_dir(runtime_ref: &str) -> String {
    let mut parts = runtime_ref.split('/');
    let name = parts.next().unwrap_or(runtime_ref);
    let arch = parts.next().unwrap_or("unknown");
    let branch = parts.next().unwrap_or("stable");
    format!("{name}-{arch}-{}", branch.replace('/', "_"))
}

pub(super) fn valid_relative_extension_path(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}
