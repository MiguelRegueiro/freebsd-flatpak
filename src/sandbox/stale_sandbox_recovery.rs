use super::application_entrypoint::sandbox_name;
use super::process_supervision::process_rooted_in;
use crate::installation as state;
use crate::installation::installation_paths::Installation;
use anyhow::{bail, Context, Result};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

pub fn recover_stale_mounts(paths: &Installation) -> Result<()> {
    state::ensure_layout(paths)?;

    for record in state::read_run_records(paths)? {
        let Some(record_path) = record.get("_path").map(PathBuf::from) else {
            continue;
        };
        let launcher_pid = record
            .get("launcher_pid")
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(0);
        let child_pid = record
            .get("child_pid")
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(0);
        let root = record.get("root").map(PathBuf::from);

        if launcher_pid > 0 && process_alive(launcher_pid) {
            continue;
        }

        if child_pid > 0 && process_alive(child_pid) {
            eprintln!("recovering stale sandbox child pid {child_pid}");
            terminate_process(child_pid);
        }
        if let Some(root) = root {
            terminate_chroot_processes(&root)?;
            unmount_under(&root)?;
            remove_instance_root(&root)?;
        }
        state::remove_run_record(&record_path)?;
    }

    // Refresh active roots after observing the mount table. A concurrent run
    // publishes its record before creating any mounts, so every mount that can
    // appear here has an ownership record visible in this second snapshot.
    let chroot_root = paths.chroots();
    let mut stale_mounts = mount_points_under(&chroot_root)?;
    let active_roots = active_run_roots(paths)?;
    stale_mounts.retain(|mountpoint| !belongs_to_any_root(mountpoint, &active_roots));
    let mut stale_roots = BTreeSet::new();
    for mountpoint in &stale_mounts {
        if let Some(root) = chroot_root_for_mount(&chroot_root, mountpoint) {
            stale_roots.insert(root);
        }
    }
    for root in stale_roots {
        terminate_chroot_processes(&root)?;
        terminate_chroot_mount_holders(&root)?;
    }
    let mut stale_mounts = mount_points_under(&chroot_root)?;
    stale_mounts.retain(|mountpoint| !belongs_to_any_root(mountpoint, &active_roots));
    unmount_mountpoints(stale_mounts)?;
    Ok(())
}

pub fn app_has_mounts(paths: &Installation, app_id: &str) -> Result<bool> {
    let root = paths.chroots().join(sandbox_name(app_id));
    Ok(!mount_points_under(&root)?.is_empty())
}

fn active_run_roots(paths: &Installation) -> Result<Vec<PathBuf>> {
    Ok(state::read_run_records(paths)?
        .into_iter()
        .filter(|record| {
            record
                .get("launcher_pid")
                .and_then(|value| value.parse::<i32>().ok())
                .is_some_and(|pid| pid > 0 && process_alive(pid))
        })
        .filter_map(|record| record.get("root").map(PathBuf::from))
        .collect())
}

pub(super) fn belongs_to_any_root(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path.starts_with(root))
}

pub(super) fn ensure_mountpoint_free(target: &Path) -> Result<()> {
    if is_mounted(target)? {
        bail!(
            "sandbox mountpoint is already mounted; clean it first: {}",
            target.display()
        );
    }
    Ok(())
}

fn is_mounted(target: &Path) -> Result<bool> {
    Ok(mount_points()?
        .iter()
        .any(|mountpoint| mountpoint == target))
}

fn mount_points_under(root: &Path) -> Result<Vec<PathBuf>> {
    let mut mounts: Vec<PathBuf> = mount_points()?
        .into_iter()
        .filter(|mountpoint| mountpoint.starts_with(root))
        .collect();
    mounts.sort_by(|left, right| {
        right
            .components()
            .count()
            .cmp(&left.components().count())
            .then_with(|| right.cmp(left))
    });
    Ok(mounts)
}

fn mount_points() -> Result<Vec<PathBuf>> {
    Ok(mount_infos()?
        .into_iter()
        .map(|mount| mount.mountpoint)
        .collect())
}

#[derive(Debug)]
struct MountInfo {
    mountpoint: PathBuf,
    options: String,
}

fn mount_infos() -> Result<Vec<MountInfo>> {
    let output = Command::new("mount").output().context("read mount table")?;
    if !output.status.success() {
        bail!("mount command failed with status {}", output.status);
    }
    let mount_table = String::from_utf8(output.stdout)?;
    Ok(mount_table
        .lines()
        .filter_map(|line| {
            line.split_once(" on ")
                .and_then(|(_, rest)| rest.split_once(" ("))
                .map(|(mountpoint, options)| MountInfo {
                    mountpoint: PathBuf::from(mountpoint),
                    options: options.trim_end_matches(')').to_string(),
                })
        })
        .collect())
}

fn unmount_under(root: &Path) -> Result<()> {
    unmount_mountpoints(mount_points_under(root)?)
}

fn terminate_chroot_mount_holders(root: &Path) -> Result<()> {
    let mut pids = BTreeSet::new();
    for mountpoint in mount_points_under(root)? {
        for pid in mount_holders(&mountpoint)? {
            if pid == std::process::id() as i32 {
                continue;
            }
            if process_rooted_in(pid, root)? {
                pids.insert(pid);
            }
        }
    }

    for pid in pids {
        eprintln!("recovering stale sandbox mount holder pid {pid}");
        terminate_process(pid);
    }
    Ok(())
}

pub(super) fn terminate_chroot_processes(root: &Path) -> Result<()> {
    let output = Command::new("ps")
        .args(["-axo", "pid"])
        .output()
        .context("list processes for sandbox cleanup")?;
    if !output.status.success() {
        bail!("ps -axo pid failed with status {}", output.status);
    }
    let text = String::from_utf8(output.stdout)?;
    let mut pids = BTreeSet::new();
    for line in text.lines().skip(1) {
        let Ok(pid) = line.trim().parse::<i32>() else {
            continue;
        };
        if pid == std::process::id() as i32 {
            continue;
        }
        if process_rooted_in(pid, root)? {
            pids.insert(pid);
        }
    }

    for pid in pids {
        eprintln!("terminating remaining sandbox process pid {pid}");
        terminate_process(pid);
    }
    Ok(())
}

fn mount_holders(mountpoint: &Path) -> Result<Vec<i32>> {
    let output = Command::new("fstat")
        .arg("-f")
        .arg(mountpoint)
        .output()
        .with_context(|| format!("find mount holders for {}", mountpoint.display()))?;
    if !output.status.success() {
        bail!(
            "fstat -f {} failed with status {}",
            mountpoint.display(),
            output.status
        );
    }
    let text = String::from_utf8(output.stdout)?;
    Ok(text
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().nth(2))
        .filter_map(|pid| pid.parse::<i32>().ok())
        .collect())
}

pub(super) fn chroot_root_for_mount(chroot_root: &Path, mountpoint: &Path) -> Option<PathBuf> {
    let mut candidate = mountpoint.parent();
    while let Some(path) = candidate {
        if path == chroot_root {
            break;
        }
        if path.starts_with(chroot_root) && path.join(".flatpak-info").is_file() {
            return Some(path.to_path_buf());
        }
        candidate = path.parent();
    }
    None
}

pub(super) fn remove_instance_root(root: &Path) -> Result<()> {
    match fs::remove_dir_all(root) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove sandbox root {}", root.display())),
    }
}

#[cfg(test)]
#[path = "tests/stale_sandbox_recovery.rs"]
mod tests;

fn unmount_mountpoints(mountpoints: Vec<PathBuf>) -> Result<()> {
    let mut errors = Vec::new();
    for mountpoint in mountpoints {
        let read_only = mountpoint_is_read_only(&mountpoint).unwrap_or(false);
        if let Err(error) = unmount_mountpoint(&mountpoint, read_only, "recover umount") {
            errors.push(format!("{error:#}"));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        bail!("stale mount recovery failed:\n{}", errors.join("\n"));
    }
}

fn mountpoint_is_read_only(mountpoint: &Path) -> Result<bool> {
    Ok(mount_infos()?
        .into_iter()
        .find(|mount| mount.mountpoint == mountpoint)
        .map(|mount| {
            mount
                .options
                .split(',')
                .map(str::trim)
                .any(|option| option == "read-only")
        })
        .unwrap_or(false))
}

pub(super) fn unmount_mountpoint(mountpoint: &Path, allow_force: bool, action: &str) -> Result<()> {
    unmount_mountpoint_with(
        mountpoint,
        allow_force,
        action,
        is_mounted,
        |path, force| {
            let mut command = Command::new("doas");
            command.arg("umount");
            if force {
                command.arg("-f");
            }
            command.arg(path);
            let force_label = if force { " -f" } else { "" };
            run_command(
                command,
                &format!("{action}{force_label} {}", path.display()),
            )
        },
        || thread::sleep(Duration::from_millis(250)),
    )
}

pub(super) fn unmount_mountpoint_with(
    mountpoint: &Path,
    allow_force: bool,
    action: &str,
    mut mounted: impl FnMut(&Path) -> Result<bool>,
    mut unmount: impl FnMut(&Path, bool) -> Result<()>,
    mut retry_pause: impl FnMut(),
) -> Result<()> {
    let mut last_error = None;
    for _ in 0..8 {
        if !mounted(mountpoint)? {
            return Ok(());
        }
        match unmount(mountpoint, false) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                if !mounted(mountpoint)? {
                    return Ok(());
                }
                retry_pause();
            }
        }
    }

    if allow_force && mounted(mountpoint)? {
        eprintln!(
            "warning: normal unmount stayed busy for read-only mount {}; trying umount -f",
            mountpoint.display()
        );
        unmount(mountpoint, true)?;
        return Ok(());
    }

    if !mounted(mountpoint)? {
        return Ok(());
    }

    let Some(error) = last_error else {
        bail!("{action} {} was not attempted", mountpoint.display());
    };
    Err(error)
}

pub(super) fn run_command(mut command: Command, action: &str) -> Result<()> {
    let status = command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| action.to_string())?;
    if !status.success() {
        bail!("{action} failed with status {status}");
    }
    Ok(())
}

fn process_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

fn terminate_process(pid: i32) {
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
    for _ in 0..20 {
        if !process_alive(pid) {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }

    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
    for _ in 0..20 {
        if !process_alive(pid) {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
}
