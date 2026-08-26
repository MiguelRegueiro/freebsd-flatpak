use super::filesystem_grants::{HostFilesystem, HostPathGrant};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub(super) struct AppDataMountPlan {
    pub(super) grants_before_app_data: Vec<HostPathGrant>,
    pub(super) mask_app_data_root: bool,
    pub(super) app_data_root: PathBuf,
    pub(super) grants_inside_app_data_root: Vec<HostPathGrant>,
}

impl AppDataMountPlan {
    pub(super) fn build(filesystem: &HostFilesystem, app_data: &Path) -> Result<Self> {
        Self::build_from_parts(filesystem.grants(), filesystem.persistent_paths(), app_data)
    }

    fn build_from_parts(
        grants: &[HostPathGrant],
        persistent_paths: &[PathBuf],
        app_data: &Path,
    ) -> Result<Self> {
        let app_data_root = app_data
            .parent()
            .context("application data path must have a parent")?
            .to_path_buf();
        let mut grants_before_app_data = Vec::new();
        let mut grants_inside_app_data_root = Vec::new();

        for grant in grants {
            if grant.sandbox_path().starts_with(&app_data_root) {
                grants_inside_app_data_root.push(grant.clone());
            } else {
                grants_before_app_data.push(grant.clone());
            }
        }

        let broad_grant_covers_app_data_root = grants_before_app_data
            .iter()
            .any(|grant| grant.maps_host_path_to_same_sandbox_path(&app_data_root));
        let relative_app_data_root = Path::new(".var/app");
        let persistent_mount_covers_app_data_root = persistent_paths
            .iter()
            .any(|path| path == Path::new(".") || relative_app_data_root.starts_with(path));

        Ok(Self {
            grants_before_app_data,
            mask_app_data_root: broad_grant_covers_app_data_root
                || persistent_mount_covers_app_data_root,
            app_data_root,
            grants_inside_app_data_root,
        })
    }
}

#[cfg(test)]
#[path = "tests/app_data_mount_plan.rs"]
mod tests;
