use super::application_entrypoint::{host_user, numeric_id};
use super::stale_sandbox_recovery::{
    ensure_mountpoint_free, remove_instance_root, run_command, unmount_mountpoint,
};
use crate::installation as state;
use crate::installation::installation_paths::Installation;
use anyhow::{bail, Context, Result};
use std::fs;
use std::os::unix::fs as unix_fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn run(
    paths: &Installation,
    checkout: &Path,
    runtime_dir: &Path,
    extra_dir: &Path,
    use_runtime: bool,
) -> Result<()> {
    if !use_runtime {
        bail!("extra-data processing without a runtime is not yet supported");
    }
    let metadata = fs::read_to_string(checkout.join("metadata"))?;
    let app_id = crate::flatpak_metadata::value(&metadata, "Application", "name")
        .or_else(|| crate::flatpak_metadata::value(&metadata, "Runtime", "name"))
        .context("extra-data checkout metadata has no application or runtime name")?;
    let uid = numeric_id("id", "-u")?;
    let gid = numeric_id("id", "-g")?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let instance_id = format!("apply-extra-{nonce:x}-{}", std::process::id());
    let root = paths.chroots().join(&app_id).join(&instance_id);
    let record =
        state::write_run_record(paths, &app_id, &instance_id, &root, std::process::id(), 0)?;
    let mut sandbox = ApplySandbox::new(root, record);
    sandbox.prepare()?;
    sandbox.mount(runtime_dir.join("files"), "usr", true)?;
    sandbox.mount(checkout.join("files"), "app", true)?;
    sandbox.mount(extra_dir.to_path_buf(), "app/extra", false)?;

    let user = host_user(uid);
    let command_text = "cd /app/extra && exec /app/bin/apply_extra";
    let mut command = Command::new("doas");
    command
        .arg("chroot")
        .arg("-n")
        .arg("-u")
        .arg(uid.to_string())
        .arg("-g")
        .arg(gid.to_string())
        .arg(&sandbox.root)
        .arg("/usr/bin/env")
        .arg("-i")
        .arg(format!("HOME=/home/{user}"))
        .arg("PATH=/app/bin:/usr/bin")
        .arg("/usr/bin/sh")
        .arg("-c")
        .arg(command_text);
    run_command(command, "run /app/bin/apply_extra")?;
    sandbox.cleanup()
}

struct ApplySandbox {
    root: PathBuf,
    record: PathBuf,
    mounts: Vec<(PathBuf, bool)>,
    cleaned: bool,
}

impl ApplySandbox {
    fn new(root: PathBuf, record: PathBuf) -> Self {
        Self {
            root,
            record,
            mounts: Vec::new(),
            cleaned: false,
        }
    }

    fn prepare(&self) -> Result<()> {
        for relative in ["app", "usr"] {
            fs::create_dir_all(self.root.join(relative))?;
        }
        for (link, target) in [
            ("bin", "usr/bin"),
            ("lib", "usr/lib"),
            ("lib64", "usr/lib64"),
        ] {
            unix_fs::symlink(target, self.root.join(link))?;
        }
        fs::write(self.root.join(".flatpak-info"), "[Instance]\n")?;
        Ok(())
    }

    fn mount(&mut self, source: PathBuf, target: &str, read_only: bool) -> Result<()> {
        let source = fs::canonicalize(&source)
            .with_context(|| format!("resolve nullfs source {}", source.display()))?;
        let target = self.root.join(target);
        fs::create_dir_all(&target)?;
        ensure_mountpoint_free(&target)?;
        let mut command = Command::new("doas");
        command.arg("mount_nullfs");
        if read_only {
            command.args(["-o", "ro"]);
        }
        command.arg(source).arg(&target);
        run_command(command, &format!("mount apply_extra {}", target.display()))?;
        self.mounts.push((target, read_only));
        Ok(())
    }

    fn cleanup(&mut self) -> Result<()> {
        let mut errors = Vec::new();
        for (mount, read_only) in self.mounts.iter().rev() {
            if let Err(error) = unmount_mountpoint(mount, *read_only, "umount apply_extra") {
                errors.push(format!("{error:#}"));
            }
        }
        if !errors.is_empty() {
            bail!("apply_extra sandbox cleanup failed:\n{}", errors.join("\n"));
        }
        self.mounts.clear();
        remove_instance_root(&self.root)?;
        state::remove_run_record(&self.record)?;
        self.cleaned = true;
        Ok(())
    }
}

impl Drop for ApplySandbox {
    fn drop(&mut self) {
        if !self.cleaned {
            if let Err(error) = self.cleanup() {
                eprintln!("warning: apply_extra sandbox cleanup failed: {error:#}");
            }
        }
    }
}
