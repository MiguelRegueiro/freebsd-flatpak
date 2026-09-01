use super::chroot_instance::{ChrootInstance, NullfsMapping, OwnedMount};
use super::stale_sandbox_recovery::{ensure_mountpoint_free, run_command};
use crate::installation::{ExtensionMergeDirectory, ExtensionMount};
use crate::secure_mount;
use anyhow::{bail, Context, Result};
use std::fs;
use std::os::unix::fs as unix_fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const STAGING_ROOT: &str = ".freebsd-flatpak-mount-sources";

fn identity(path: &Path) -> Result<(u64, u64)> {
    let metadata =
        fs::metadata(path).with_context(|| format!("read metadata for {}", path.display()))?;
    Ok((metadata.dev(), metadata.ino()))
}

impl ChrootInstance {
    pub(super) fn prepare_extension_target(
        &mut self,
        extension: &ExtensionMount,
        app_dir: &Path,
        runtime_dir: &Path,
    ) -> Result<()> {
        self.prepare_extension_directory(&extension.target, app_dir, runtime_dir)
    }

    pub(super) fn prepare_extension_merge_target(
        &mut self,
        merge: &ExtensionMergeDirectory,
        app_dir: &Path,
        runtime_dir: &Path,
    ) -> Result<()> {
        self.prepare_extension_directory(&merge.target, app_dir, runtime_dir)
    }

    fn prepare_extension_directory(
        &mut self,
        target_relative: &Path,
        app_dir: &Path,
        runtime_dir: &Path,
    ) -> Result<()> {
        let target = self.root.join(target_relative);
        if target.is_dir() {
            return Ok(());
        }
        if fs::create_dir_all(&target).is_ok() {
            return Ok(());
        }
        let (scope, deployment) = if target_relative.starts_with("app") {
            (Path::new("app"), app_dir)
        } else if target_relative.starts_with("usr") {
            (Path::new("usr"), runtime_dir)
        } else {
            bail!(
                "extension target is outside /app and /usr: {}",
                target_relative.display()
            );
        };
        let mut overlay = target_relative
            .parent()
            .context("extension target has no parent")?
            .to_path_buf();
        while !self.root.join(&overlay).is_dir() {
            if !overlay.pop() || overlay == scope {
                bail!(
                    "extension target has no safe existing parent: {}",
                    target_relative.display()
                );
            }
        }
        let relative = overlay
            .strip_prefix(scope)
            .expect("extension overlay remains inside scope")
            .to_path_buf();
        if overlay == scope {
            bail!(
                "refusing to overlay complete extension scope for target {}",
                target_relative.display()
            );
        }
        let placeholders = self
            .extension_plan
            .mounts
            .iter()
            .filter_map(|mount| {
                mount
                    .target
                    .strip_prefix(&overlay)
                    .ok()?
                    .components()
                    .next()
                    .map(|component| PathBuf::from(component.as_os_str()))
            })
            .collect();
        self.mount_extension_merge_inner(
            &ExtensionMergeDirectory {
                target: overlay,
                base_source: deployment.join("files").join(relative),
                entries: Vec::new(),
            },
            &placeholders,
        )?;
        fs::create_dir_all(&target)
            .with_context(|| format!("create sandbox extension target {}", target.display()))
    }

    pub(super) fn mount_extension_merge(&mut self, merge: &ExtensionMergeDirectory) -> Result<()> {
        self.mount_extension_merge_inner(merge, &std::collections::BTreeSet::new())
    }

    fn mount_extension_merge_inner(
        &mut self,
        merge: &ExtensionMergeDirectory,
        placeholders: &std::collections::BTreeSet<PathBuf>,
    ) -> Result<()> {
        let target = self.root.join(&merge.target);
        if !target.is_dir() {
            bail!(
                "extension merge target is missing from its deployment: {}",
                target.display()
            );
        }
        let selected = merge
            .entries
            .iter()
            .map(|entry| entry.name.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let mut sources = Vec::new();
        if let Ok(base_entries) = fs::read_dir(&merge.base_source) {
            for entry in base_entries {
                let entry = entry
                    .with_context(|| format!("read merge base {}", merge.base_source.display()))?;
                let name = PathBuf::from(entry.file_name());
                if !selected.contains(&name) && !placeholders.contains(&name) {
                    sources.push((name, entry.path()));
                }
            }
        }
        sources.extend(
            merge
                .entries
                .iter()
                .map(|entry| (entry.name.clone(), entry.source.clone())),
        );
        sources.sort_by(|left, right| left.0.cmp(&right.0));

        self.mount_tmpfs_secure(&merge.target, "mode=0755")?;
        for name in placeholders {
            fs::create_dir(target.join(name)).with_context(|| {
                format!(
                    "create extension placeholder {}",
                    target.join(name).display()
                )
            })?;
        }
        for (name, source) in sources {
            let destination = target.join(&name);
            let metadata = fs::symlink_metadata(&source)
                .with_context(|| format!("inspect merge source {}", source.display()))?;
            if metadata.file_type().is_symlink() {
                let link = fs::read_link(&source)
                    .with_context(|| format!("read merge symlink {}", source.display()))?;
                unix_fs::symlink(&link, &destination).with_context(|| {
                    format!(
                        "create merged symlink {} -> {}",
                        destination.display(),
                        link.display()
                    )
                })?;
            } else {
                if metadata.is_dir() {
                    fs::create_dir(&destination)
                        .with_context(|| format!("create {}", destination.display()))?;
                } else {
                    fs::File::create(&destination)
                        .with_context(|| format!("create {}", destination.display()))?;
                }
                self.mount_nullfs(&source, merge.target.join(name), true)?;
            }
        }
        Ok(())
    }

    pub(super) fn mount_nullfs(
        &mut self,
        source: &Path,
        target_relative: impl AsRef<Path>,
        read_only: bool,
    ) -> Result<()> {
        self.mount_nullfs_impl(source, target_relative.as_ref(), read_only, false)
    }

    pub(super) fn mount_nullfs_secure(
        &mut self,
        source: &Path,
        target_relative: impl AsRef<Path>,
        read_only: bool,
    ) -> Result<()> {
        self.mount_nullfs_impl(source, target_relative.as_ref(), read_only, true)
    }

    pub(super) fn mount_nullfs_extra_permission(
        &mut self,
        source: &Path,
        target_relative: impl AsRef<Path>,
        read_only: bool,
    ) -> Result<()> {
        let target_relative = target_relative.as_ref();
        self.mount_nullfs_impl(source, target_relative, read_only, true)?;
        self.nested_excluded_mounts
            .push(self.root.join(target_relative));
        Ok(())
    }
    fn mount_nullfs_impl(
        &mut self,
        source: &Path,
        target_relative: &Path,
        read_only: bool,
        _secure_target: bool,
    ) -> Result<()> {
        let source_is_private = source.starts_with(self.root.join(STAGING_ROOT));
        let source = if source_is_private {
            source.to_path_buf()
        } else {
            fs::canonicalize(source)
                .with_context(|| format!("resolve nullfs source {}", source.display()))?
        };
        let target_relative = target_relative.to_path_buf();
        let target = self.root.join(&target_relative);
        ensure_mountpoint_free(&target)?;

        let aliases_parent =
            nullfs_source_aliases_parent(&self.nullfs_mounts, &self.owned_mounts, &source, &target);
        if aliases_parent {
            if !self.mount_staging_ready {
                let staging_root = self.root.join(STAGING_ROOT);
                fs::create_dir(&staging_root).with_context(|| {
                    format!(
                        "create private mount staging root {}",
                        staging_root.display()
                    )
                })?;
                self.mount_tmpfs_secure(STAGING_ROOT, "mode=0700")?;
                self.mount_staging_ready = true;
            }
            let staging_relative =
                Path::new(STAGING_ROOT).join(self.next_mount_staging_id.to_string());
            self.next_mount_staging_id += 1;
            self.mount_nullfs_impl(&source, &staging_relative, read_only, true)?;
            let staging_source = self.root.join(staging_relative);
            self.mount_nullfs_impl(&staging_source, &target_relative, read_only, true)?;
            if let Some(mapping) = self.nullfs_mounts.last_mut() {
                mapping.source = source;
            }
            return Ok(());
        }

        let command = secure_mount::nullfs_command(
            &self.root,
            identity(&self.root)?,
            &source,
            if source_is_private {
                None
            } else {
                Some(identity(&source)?)
            },
            &target_relative,
            read_only,
        )?;
        run_command(command, &format!("mount nullfs {}", target.display()))?;
        self.owned_mounts.push(OwnedMount {
            path: target.clone(),
            read_only,
        });
        self.nullfs_mounts.push(NullfsMapping { source, target });
        Ok(())
    }

    pub(super) fn mount_special(
        &mut self,
        target_relative: impl AsRef<Path>,
        fs_type: &str,
        source: &str,
    ) -> Result<()> {
        let target = self.root.join(target_relative.as_ref());
        ensure_mountpoint_free(&target)?;

        let command = secure_mount::special_command(
            &self.root,
            identity(&self.root)?,
            target_relative.as_ref(),
            fs_type,
            source,
        )?;
        run_command(command, &format!("mount {fs_type} {}", target.display()))?;
        self.owned_mounts.push(OwnedMount {
            path: target,
            read_only: false,
        });
        Ok(())
    }

    pub(super) fn mount_tmpfs(
        &mut self,
        target_relative: impl AsRef<Path>,
        options: &str,
    ) -> Result<()> {
        let target_relative = target_relative.as_ref();
        let target = self.root.join(target_relative);
        ensure_mountpoint_free(&target)?;
        let command = secure_mount::tmpfs_command(
            &self.root,
            identity(&self.root)?,
            target_relative,
            options,
        )?;
        run_command(command, &format!("mount tmpfs {}", target.display()))?;
        self.owned_mounts.push(OwnedMount {
            path: target,
            read_only: false,
        });
        Ok(())
    }

    pub(super) fn mount_tmpfs_secure(
        &mut self,
        target_relative: impl AsRef<Path>,
        options: &str,
    ) -> Result<()> {
        let target_relative = target_relative.as_ref();
        let target = self.root.join(target_relative);
        ensure_mountpoint_free(&target)?;

        let command = secure_mount::tmpfs_command(
            &self.root,
            identity(&self.root)?,
            target_relative,
            options,
        )?;
        run_command(command, &format!("mount tmpfs {}", target.display()))?;
        self.owned_mounts.push(OwnedMount {
            path: target,
            read_only: false,
        });
        Ok(())
    }
}

fn nullfs_source_aliases_parent(
    mappings: &[NullfsMapping],
    mounts: &[OwnedMount],
    source: &Path,
    target: &Path,
) -> bool {
    let Some(covering_mount) = mounts
        .iter()
        .filter(|mount| target != mount.path && target.starts_with(&mount.path))
        .max_by_key(|mount| mount.path.components().count())
    else {
        return false;
    };
    let Some(mapping) = mappings
        .iter()
        .rev()
        .find(|mapping| mapping.target == covering_mount.path)
    else {
        return false;
    };
    let Ok(suffix) = target.strip_prefix(&mapping.target) else {
        return false;
    };
    fs::canonicalize(mapping.source.join(suffix)).is_ok_and(|mapped| mapped == source)
}

pub(super) fn owned_mount_teardown_order(
    root: &Path,
    mut mounts: Vec<OwnedMount>,
) -> Result<Vec<OwnedMount>> {
    if let Some(mount) = mounts.iter().find(|mount| !mount.path.starts_with(root)) {
        bail!(
            "refusing to clean mount outside sandbox root {}: {}",
            root.display(),
            mount.path.display()
        );
    }
    mounts.sort_by(|left, right| {
        right
            .path
            .components()
            .count()
            .cmp(&left.path.components().count())
            .then_with(|| right.path.cmp(&left.path))
    });
    Ok(mounts)
}

#[cfg(test)]
#[path = "tests/mount_operations.rs"]
mod tests;
