use super::application_records::list_apps;
use super::installation_paths::Installation;
use super::run_records::read_run_records;
use super::runtime_records::list_runtimes;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub(super) struct DeploymentData {
    pub ref_name: String,
    pub commit: String,
    pub installed_size: u64,
    pub origin: String,
}

pub fn cleanup_retired_deployments(paths: &Installation) -> Result<Vec<PathBuf>> {
    let mut protected = std::collections::BTreeSet::new();
    for app in list_apps(paths)? {
        protected.insert(absolute(paths, &app.app_dir));
        protected.insert(absolute(paths, &app.runtime_dir));
    }
    for runtime in list_runtimes(paths)? {
        protected.insert(absolute(paths, &runtime.runtime_dir));
    }
    for run in read_run_records(paths)? {
        for key in ["app_dir", "runtime_dir"] {
            if let Some(path) = run.get(key) {
                protected.insert(absolute(paths, Path::new(path)));
            }
        }
    }

    let mut removed = Vec::new();
    for root in [paths.apps(), paths.runtimes()] {
        if !root.is_dir() {
            continue;
        }
        for family in fs::read_dir(&root).with_context(|| format!("read {}", root.display()))? {
            let family = family?;
            if !family.file_type()?.is_dir() {
                continue;
            }
            let family_path = family.path();
            // Legacy checkouts live directly at `family`; generations always
            // live one level below it and are safe to identify independently.
            for generation in fs::read_dir(&family_path)
                .with_context(|| format!("read {}", family_path.display()))?
            {
                let generation = generation?;
                let path = generation.path();
                if !generation.file_type()?.is_dir()
                    || !path.join(".ostree-commit").is_file()
                    || protected.contains(&path)
                {
                    continue;
                }
                safe_remove_dir(paths, &path)?;
                removed.push(path);
            }
            // Once a legacy checkout is superseded, remove its payload while
            // retaining any commit-qualified children created beneath the
            // former checkout directory.
            if family_path.join(".ostree-commit").is_file() && !protected.contains(&family_path) {
                for entry in fs::read_dir(&family_path)
                    .with_context(|| format!("read {}", family_path.display()))?
                {
                    let entry = entry?;
                    let path = entry.path();
                    if entry.file_type()?.is_dir() && path.join(".ostree-commit").is_file() {
                        continue;
                    }
                    remove_managed_path(paths, &path)?;
                }
                removed.push(family_path);
            }
        }
    }
    Ok(removed)
}

fn remove_managed_path(paths: &Installation, path: &Path) -> Result<()> {
    let allowed = [paths.apps(), paths.runtimes(), paths.extensions()];
    if !allowed
        .iter()
        .any(|root| path.starts_with(root) && path != root)
    {
        bail!("refusing to remove unmanaged path: {}", path.display());
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            fs::remove_dir_all(path).with_context(|| format!("remove {}", path.display()))
        }
        Ok(_) => fs::remove_file(path).with_context(|| format!("remove {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

pub(crate) fn deployment_marker(path: &Path) -> Result<Option<(String, String)>> {
    let marker_path = path.join(".ostree-commit");
    if !marker_path.is_file() {
        return Ok(None);
    }
    let marker = fs::read_to_string(&marker_path)
        .with_context(|| format!("read {}", marker_path.display()))?;
    let mut lines = marker.lines();
    let ref_name = lines.next().context("deployment marker missing ref")?;
    let commit = lines.next().context("deployment marker missing commit")?;
    Ok(Some((ref_name.to_string(), commit.to_string())))
}

pub(super) fn deployment_data(path: &Path) -> Result<Option<DeploymentData>> {
    let marker_path = path.join(".ostree-commit");
    if !marker_path.is_file() {
        return Ok(None);
    }
    let marker = fs::read_to_string(&marker_path)
        .with_context(|| format!("read {}", marker_path.display()))?;
    let mut lines = marker.lines();
    let ref_name = lines.next().context("deployment marker missing ref")?;
    let commit = lines.next().context("deployment marker missing commit")?;
    let installed_size = lines
        .next()
        .context("deployment marker missing installed size")?
        .parse()
        .context("deployment marker has invalid installed size")?;
    let origin = lines
        .next()
        .context("deployment marker missing origin")?
        .to_string();
    Ok(Some(DeploymentData {
        ref_name: ref_name.to_string(),
        commit: commit.to_string(),
        installed_size,
        origin,
    }))
}

pub fn checkout_ref(path: &Path) -> Result<Option<String>> {
    Ok(deployment_marker(path)?.map(|(ref_name, _)| ref_name))
}

pub fn absolute(paths: &Installation, path: &Path) -> PathBuf {
    paths.absolute_data_path(path)
}

pub fn safe_remove_dir(paths: &Installation, path: &Path) -> Result<()> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        paths.absolute_data_path(path)
    };
    if !path.exists() {
        return Ok(());
    }

    let allowed = [
        paths.apps(),
        paths.runtimes(),
        paths.chroots(),
        paths.extensions(),
    ];
    if !allowed
        .iter()
        .any(|root| path.starts_with(root) && path != *root)
    {
        bail!(
            "refusing to remove path outside managed runtime data: {}",
            path.display()
        );
    }

    fs::remove_dir_all(&path).with_context(|| format!("remove {}", path.display()))
}

#[cfg(test)]
#[path = "tests/generation_cleanup.rs"]
mod tests;
