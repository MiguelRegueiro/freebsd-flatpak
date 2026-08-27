use super::filesystem_grants::{HostFilesystem, HostPathGrant};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Orders mounts around Flatpak's protected per-application data namespace.
///
/// A broad `home` or `host` grant must not make other applications' data
/// visible. On FreeBSD, overlaying the protected root with tmpfs provides the
/// missing namespace boundary; the current application's data and explicit
/// cross-application grants are then mounted back below it.
pub(super) struct FlatpakDataMountPlan {
    pub(super) grants_before_mask: Vec<HostPathGrant>,
    pub(super) mask_app_data_root: bool,
    pub(super) app_data_root: PathBuf,
    pub(super) app_data: PathBuf,
    pub(super) reserved_roots_to_mask: Vec<PathBuf>,
    pub(super) grants_after_mask: Vec<HostPathGrant>,
}

impl FlatpakDataMountPlan {
    pub(super) fn build(
        filesystem: &HostFilesystem,
        app_data: &Path,
        reserved_roots: &[PathBuf],
    ) -> Result<Self> {
        Self::build_from_parts(filesystem.grants(), app_data, reserved_roots)
    }

    #[cfg(test)]
    fn build_from_grants(grants: &[HostPathGrant], app_data: &Path) -> Result<Self> {
        Self::build_from_parts(grants, app_data, &[])
    }

    fn build_from_parts(
        grants: &[HostPathGrant],
        app_data: &Path,
        reserved_roots: &[PathBuf],
    ) -> Result<Self> {
        let app_data_root = app_data
            .parent()
            .context("application data path must have a parent")?
            .to_path_buf();
        let mut grants_before_mask = Vec::new();
        let mut grants_after_mask = Vec::new();

        let mut protected_roots = reserved_roots.to_vec();
        protected_roots.push(app_data_root.clone());
        protected_roots.sort();
        protected_roots.dedup();

        for grant in grants {
            if grant.sandbox_path().starts_with(app_data) {
                // The canonical app-data mount already exposes every path in
                // the current application's own tree. Re-mounting one of
                // those paths onto itself can produce EDEADLK on nullfs.
            } else if protected_roots
                .iter()
                .any(|root| grant.sandbox_path().starts_with(root))
            {
                grants_after_mask.push(grant.clone());
            } else {
                grants_before_mask.push(grant.clone());
            }
        }

        let mask_app_data_root = grants_before_mask
            .iter()
            .any(|grant| grant.maps_host_path_to_same_sandbox_path(&app_data_root));
        let mut reserved_roots_to_mask = reserved_roots
            .iter()
            .filter(|root| {
                grants_before_mask
                    .iter()
                    .any(|grant| grant.maps_host_path_to_same_sandbox_path(root))
            })
            .cloned()
            .collect::<Vec<_>>();
        reserved_roots_to_mask.sort();
        reserved_roots_to_mask.dedup();

        Ok(Self {
            grants_before_mask,
            mask_app_data_root,
            app_data_root,
            app_data: app_data.to_path_buf(),
            reserved_roots_to_mask,
            grants_after_mask,
        })
    }
}

#[cfg(test)]
#[path = "tests/flatpak_data_mount_plan.rs"]
mod tests;
