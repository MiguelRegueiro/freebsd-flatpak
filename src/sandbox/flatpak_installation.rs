use crate::installation::{self as state, installation_paths::Installation};
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

const PROJECTION_SOURCE: &str = ".freebsd-flatpak-mount-sources/flatpak-apps";

pub(super) struct FlatpakInstallationProjection {
    pub(super) source_root: PathBuf,
    pub(super) target_root: PathBuf,
    pub(super) deployments: Vec<ProjectedDeployment>,
}

pub(super) struct ProjectedDeployment {
    pub(super) source: PathBuf,
    pub(super) target: PathBuf,
}

impl FlatpakInstallationProjection {
    pub(super) fn prepare(root: &Path, paths: &Installation) -> Result<Self> {
        let target_root = paths.data_home().join("flatpak/app");
        let target_root = target_root
            .strip_prefix(Path::new("/"))
            .with_context(|| format!("XDG data home must be absolute: {}", target_root.display()))?
            .to_path_buf();
        let source_root = root.join(PROJECTION_SOURCE);
        fs::create_dir_all(&source_root)
            .with_context(|| format!("create {}", source_root.display()))?;

        let mut deployments = Vec::new();
        for record in state::list_apps(paths)? {
            if record.app_id.is_empty() || record.app_id.contains('/') {
                bail!("invalid installed application id: {:?}", record.app_id);
            }
            let relative = PathBuf::from(&record.app_id).join("current/active");
            let skeleton = source_root.join(&relative);
            fs::create_dir_all(&skeleton)
                .with_context(|| format!("create {}", skeleton.display()))?;
            deployments.push(ProjectedDeployment {
                source: state::absolute(paths, &record.app_dir),
                target: target_root.join(relative),
            });
        }

        Ok(Self {
            source_root,
            target_root,
            deployments,
        })
    }
}

#[cfg(test)]
#[path = "tests/flatpak_installation.rs"]
mod tests;
