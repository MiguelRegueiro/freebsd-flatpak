use super::sandbox_portal::PortalProxy;
use super::stale_portal_recovery::{process_alive, remove_dir, terminate_process, unmount_under};
use crate::installation::installation_paths::Installation;
use anyhow::{bail, Context, Result};
use std::fs;
use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) fn shared_portal_dir(paths: &Installation, app_id: &str) -> PathBuf {
    paths.portal().join("apps").join(app_scope_name(app_id))
}

pub(super) fn app_scope_name(app_id: &str) -> String {
    app_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub(super) fn portal_control(proxy: &PortalProxy, method: &str) -> Result<()> {
    portal_control_args(
        proxy,
        method,
        &[proxy.sandbox_doc_dir.display().to_string()],
    )
}

pub(super) fn portal_control_args(
    proxy: &PortalProxy,
    method: &str,
    args: &[String],
) -> Result<()> {
    let output = Command::new("gdbus")
        .arg("call")
        .arg("--address")
        .arg(&proxy.private_bus_address)
        .arg("--dest")
        .arg("org.freedesktop.portal.Desktop")
        .arg("--object-path")
        .arg("/org/freebsd/Flatpak/PortalBridge")
        .arg("--method")
        .arg(format!("org.freebsd.Flatpak.PortalBridge.{method}"))
        .args(args)
        .output()
        .with_context(|| format!("call shared portal {method}"))?;
    if !output.status.success() {
        bail!(
            "shared portal {method} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

pub(super) fn lock_portal_scope(path: &Path) -> Result<fs::File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("open portal lock {}", path.display()))?;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if result != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("lock portal scope {}", path.display()));
    }
    Ok(file)
}

pub(super) fn stop_shared_portal(shared_dir: &Path) -> Result<()> {
    for name in [
        "portal-bridge.pid",
        "status-notifier-bridge.pid",
        "bridge.pid",
        "bus.pid",
    ] {
        let path = shared_dir.join(name);
        if let Ok(pid) = fs::read_to_string(&path) {
            if let Ok(pid) = pid.trim().parse::<i32>() {
                if process_command_contains(pid, shared_dir)? {
                    terminate_process(pid);
                }
            }
        }
    }
    unmount_under(&shared_dir.join("doc"))?;
    remove_dir(shared_dir)
}

fn process_command_contains(pid: i32, path: &Path) -> Result<bool> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .with_context(|| format!("inspect portal process {pid}"))?;
    if !output.status.success() {
        return Ok(false);
    }
    Ok(String::from_utf8(output.stdout)
        .context("portal process command is not UTF-8")?
        .contains(&path.display().to_string()))
}

pub(super) fn other_active_app_instances(
    paths: &Installation,
    app_id: &str,
    instance_id: &str,
) -> Result<bool> {
    app_has_active_run(paths, app_id, Some(instance_id))
}

pub(super) fn app_has_active_run(
    paths: &Installation,
    app_id: &str,
    excluded_instance: Option<&str>,
) -> Result<bool> {
    for record in crate::installation::read_run_records(paths)? {
        if record.get("app_id").map(String::as_str) != Some(app_id)
            || excluded_instance.is_some_and(|excluded| {
                record.get("instance_id").map(String::as_str) == Some(excluded)
            })
        {
            continue;
        }
        let pid = record
            .get("launcher_pid")
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(0);
        if pid > 0 && process_alive(pid) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn ensure_bridge_helpers(paths: &Installation) -> Result<(PathBuf, PathBuf)> {
    let portal = paths.libexec_root().join("portal-bridge");
    let status_notifier = paths.libexec_root().join("status-notifier-bridge");
    let spawn_agent = paths.libexec_root().join("sandbox-spawn-agent-linux");
    let signalfd_compat = paths.libexec_root().join("libsignalfd-compat.so");
    for output in [&portal, &status_notifier, &spawn_agent, &signalfd_compat] {
        if !output.is_file() {
            bail!(
                "installed compatibility helper is missing: {}",
                output.display()
            );
        }
    }
    Ok((portal, status_notifier))
}

#[cfg(test)]
#[path = "tests/portal_scope.rs"]
mod tests;
