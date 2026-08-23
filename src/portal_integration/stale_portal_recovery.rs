use super::portal_scope::{
    app_has_active_run, app_scope_name, lock_portal_scope, stop_shared_portal,
};
use crate::installation::installation_paths::Installation;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

pub fn recover_stale_portal_mounts(paths: &Installation) -> Result<()> {
    let doc_root = paths.portal().join("doc");
    if doc_root.is_dir() {
        for entry in fs::read_dir(&doc_root)
            .with_context(|| format!("read portal document root {}", doc_root.display()))?
        {
            let path = entry?.path();
            if path.is_dir() && !belongs_to_active_run(paths, &path)? {
                unmount_under(&path)?;
                kill_processes_referencing(&path)?;
                remove_doc_dir(&path)?;
            }
        }
    }

    {
        let bus_root = paths.portal().join("bus");
        if bus_root.is_dir() {
            for entry in fs::read_dir(&bus_root)
                .with_context(|| format!("read portal bus root {}", bus_root.display()))?
            {
                let path = entry?.path();
                if path.is_dir() && !belongs_to_active_run(paths, &path)? {
                    kill_processes_referencing(&path)?;
                    remove_dir(&path)?;
                }
            }
        }
    }
    let apps_root = paths.portal().join("apps");
    if apps_root.is_dir() {
        for entry in fs::read_dir(&apps_root)
            .with_context(|| format!("read shared portal root {}", apps_root.display()))?
        {
            let shared_dir = entry?.path();
            if !shared_dir.is_dir() {
                continue;
            }
            let app_id = fs::read_to_string(shared_dir.join("app-id"))
                .unwrap_or_default()
                .trim()
                .to_string();
            if app_id.is_empty() || !app_has_active_run(paths, &app_id, None)? {
                let lock_path = paths
                    .portal()
                    .join("locks")
                    .join(format!("{}.lock", app_scope_name(&app_id)));
                let _lock = lock_portal_scope(&lock_path)?;
                if app_id.is_empty() || !app_has_active_run(paths, &app_id, None)? {
                    stop_shared_portal(&shared_dir)?;
                }
            }
        }
    }
    Ok(())
}

pub(super) fn unmount_under(root: &Path) -> Result<()> {
    let mut mountpoints = mount_points_under(root)?;
    mountpoints.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for mountpoint in mountpoints {
        if !mount_is_present(&mountpoint)? {
            continue;
        }
        if let Err(error) = unmount_one(&mountpoint, false) {
            if !mount_is_present(&mountpoint)? {
                continue;
            }
            eprintln!(
                "warning: portal umount failed for {}: {error:#}",
                mountpoint.display()
            );
            if mount_is_present(&mountpoint)? {
                unmount_one(&mountpoint, true)?;
            }
        }
    }
    Ok(())
}

fn mount_is_present(path: &Path) -> Result<bool> {
    Ok(mount_points_under(path)?
        .into_iter()
        .any(|mountpoint| mountpoint == path))
}

fn mount_points_under(root: &Path) -> Result<Vec<PathBuf>> {
    let output = Command::new("mount").output().context("list mounts")?;
    if !output.status.success() {
        bail!("mount failed with status {}", output.status);
    }
    let text = String::from_utf8(output.stdout).context("mount output is not UTF-8")?;
    let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut mountpoints = Vec::new();
    for line in text.lines() {
        let Some((_, rest)) = line.split_once(" on ") else {
            continue;
        };
        let Some((path, _)) = rest.rsplit_once(" (") else {
            continue;
        };
        let path = PathBuf::from(path);
        if path.starts_with(&root) {
            mountpoints.push(path);
        }
    }
    Ok(mountpoints)
}

fn unmount_one(path: &Path, force: bool) -> Result<()> {
    let mut command = Command::new("doas");
    command.arg("umount");
    if force {
        command.arg("-f");
    }
    command.arg(path);
    let status = command
        .status()
        .with_context(|| format!("umount {}", path.display()))?;
    if !status.success() {
        bail!("umount {} failed with status {}", path.display(), status);
    }
    Ok(())
}

fn remove_doc_dir(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

pub(super) fn remove_dir(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

fn belongs_to_active_run(paths: &Installation, path: &Path) -> Result<bool> {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(false);
    };
    for record in crate::installation::read_run_records(paths)? {
        let pid = record
            .get("launcher_pid")
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(0);
        if pid <= 0 || !process_alive(pid) {
            continue;
        }
        if record
            .get("instance_id")
            .is_some_and(|instance_id| name.ends_with(&format!("-{}", sanitize_id(instance_id))))
            || name.ends_with(&format!("-{pid}"))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn process_alive(pid: i32) -> bool {
    unsafe {
        libc::kill(pid, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

fn kill_processes_referencing(path: &Path) -> Result<()> {
    let output = Command::new("ps")
        .args(["axww", "-o", "pid=", "-o", "command="])
        .output()
        .context("list processes for portal recovery")?;
    if !output.status.success() {
        bail!("ps failed with status {}", output.status);
    }
    let needle = path.display().to_string();
    let text = String::from_utf8(output.stdout).context("ps output is not UTF-8")?;
    for line in text.lines() {
        if !line.contains(&needle) {
            continue;
        }
        let Some(pid) = line
            .split_whitespace()
            .next()
            .and_then(|pid| pid.parse::<i32>().ok())
        else {
            continue;
        };
        terminate_process(pid);
    }
    Ok(())
}

pub(super) fn terminate_process(pid: i32) {
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
}
