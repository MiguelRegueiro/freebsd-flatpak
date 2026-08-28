use super::filesystem_grants::{HostFilesystem, HostPathGrant};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Orders mounts around Flatpak's protected data namespaces.
///
/// A broad `home`, `host`, or XDG data grant must not expose Flatpak's own
/// installation state or other applications' data. On FreeBSD, overlaying the
/// protected roots with tmpfs provides the missing namespace boundaries;
/// explicitly granted paths are then mounted back below them.
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
        data_home: &Path,
        data_root: &Path,
    ) -> Result<Self> {
        Self::build_from_installation_parts(filesystem.grants(), app_data, data_home, data_root)
    }

    fn build_from_installation_parts(
        grants: &[HostPathGrant],
        app_data: &Path,
        data_home: &Path,
        data_root: &Path,
    ) -> Result<Self> {
        // Reserve these paths regardless of whether they exist yet. Otherwise
        // an application with broad writable access could create Flatpak state
        // which a later launch would trust and consume.
        let flatpak_root = data_home.join("flatpak");
        let app_data_root = app_data
            .parent()
            .context("application data path must have a parent")?;
        let reserved_roots = [flatpak_root.clone(), data_root.to_path_buf()];
        let mut alias_roots = Vec::new();
        for path in [
            flatpak_root.as_path(),
            flatpak_root.join("overrides").as_path(),
            data_root,
            app_data_root,
        ] {
            add_canonical_alias(&mut alias_roots, path);
        }
        Self::build_with_aliases(grants, app_data, &reserved_roots, &alias_roots)
    }

    #[cfg(test)]
    fn build_from_grants(grants: &[HostPathGrant], app_data: &Path) -> Result<Self> {
        Self::build_from_parts(grants, app_data, &[])
    }

    #[cfg(test)]
    fn build_from_parts(
        grants: &[HostPathGrant],
        app_data: &Path,
        reserved_roots: &[PathBuf],
    ) -> Result<Self> {
        Self::build_with_aliases(grants, app_data, reserved_roots, &[])
    }

    fn build_with_aliases(
        grants: &[HostPathGrant],
        app_data: &Path,
        reserved_roots: &[PathBuf],
        alias_roots: &[PathBuf],
    ) -> Result<Self> {
        let app_data_root = app_data
            .parent()
            .context("application data path must have a parent")?
            .to_path_buf();
        let mut grants_before_mask = Vec::new();
        let mut grants_after_mask = Vec::new();

        // Only standard lexical paths authorize a regrant. Canonical aliases
        // are mask-only so a broad grant expanded onto an alias cannot regain
        // access to the backing directory after it is hidden.
        let mut regrant_roots = reserved_roots.to_vec();
        regrant_roots.push(app_data_root.clone());
        regrant_roots.sort();
        regrant_roots.dedup();

        for grant in grants {
            if grant.sandbox_path().starts_with(app_data) {
                // The canonical app-data mount already exposes every path in
                // the current application's own tree. Re-mounting one of
                // those paths onto itself can produce EDEADLK on nullfs.
            } else if regrant_roots
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
            .any(|grant| paths_overlap(grant.sandbox_path(), &app_data_root));
        let mut mask_roots = reserved_roots.to_vec();
        mask_roots.extend_from_slice(alias_roots);
        let mut reserved_roots_to_mask = mask_roots
            .iter()
            .filter(|root| {
                grants_before_mask
                    .iter()
                    .any(|grant| paths_overlap(grant.sandbox_path(), root))
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

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn add_canonical_alias(roots: &mut Vec<PathBuf>, path: &Path) {
    if let Ok(alias) = fs::canonicalize(path) {
        if alias != path {
            roots.push(alias);
        }
    }
}

#[cfg(test)]
#[path = "tests/flatpak_data_mount_plan.rs"]
mod tests;
