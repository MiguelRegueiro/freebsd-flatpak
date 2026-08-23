use super::chroot_instance::{ChrootInstance, OwnedMount};
use super::stale_sandbox_recovery::{ensure_mountpoint_free, run_command};
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;
use std::process::Command;

impl ChrootInstance {
    pub(super) fn mount_nullfs(
        &mut self,
        source: &Path,
        target_relative: impl AsRef<Path>,
        read_only: bool,
    ) -> Result<()> {
        let source = fs::canonicalize(source)
            .with_context(|| format!("resolve nullfs source {}", source.display()))?;
        let target = self.root.join(target_relative);
        fs::create_dir_all(&target)
            .with_context(|| format!("create mount target {}", target.display()))?;
        ensure_mountpoint_free(&target)?;

        let mut command = Command::new("doas");
        command.arg("mount_nullfs");
        if read_only {
            command.arg("-o").arg("ro");
        }
        command.arg(&source).arg(&target);
        run_command(command, &format!("mount nullfs {}", target.display()))?;
        self.owned_mounts.push(OwnedMount {
            path: target,
            read_only,
        });
        Ok(())
    }

    pub(super) fn mount_special(
        &mut self,
        target_relative: impl AsRef<Path>,
        fs_type: &str,
        source: &str,
    ) -> Result<()> {
        let target = self.root.join(target_relative);
        ensure_mountpoint_free(&target)?;

        let mut command = Command::new("doas");
        command
            .arg("mount")
            .arg("-t")
            .arg(fs_type)
            .arg(source)
            .arg(&target);
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
        let target = self.root.join(target_relative);

        let mut mkdir = Command::new("doas");
        mkdir.arg("mkdir").arg("-p").arg(&target);
        run_command(
            mkdir,
            &format!("create tmpfs mount target {}", target.display()),
        )?;
        ensure_mountpoint_free(&target)?;

        let mut command = Command::new("doas");
        command
            .arg("mount")
            .arg("-t")
            .arg("tmpfs")
            .arg("-o")
            .arg(options)
            .arg("tmpfs")
            .arg(&target);
        run_command(command, &format!("mount tmpfs {}", target.display()))?;
        self.owned_mounts.push(OwnedMount {
            path: target,
            read_only: false,
        });
        Ok(())
    }
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
