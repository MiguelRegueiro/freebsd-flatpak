use crate::desktop::DesktopSession;
use crate::paths::Installation;
use anyhow::{bail, Context, Result};
use std::fs;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

#[derive(Debug)]
pub struct HostPortal {
    proxy: Option<PortalProxy>,
    mode: PortalMode,
    warnings: Vec<String>,
}

#[derive(Debug)]
struct PortalProxy {
    paths: Installation,
    app_id: String,
    instance_id: String,
    shared_dir: PathBuf,
    doc_dir: PathBuf,
    sandbox_doc_dir: PathBuf,
    private_bus_address: String,
    sandbox_bus_address: String,
}

#[derive(Debug, Clone, Copy)]
enum PortalMode {
    PrivateProxy,
    Disabled,
}

impl HostPortal {
    pub fn prepare(
        paths: &Installation,
        app_id: &str,
        instance_id: &str,
        desktop: &DesktopSession,
        uid: u32,
        sandbox_root: &Path,
    ) -> Result<Self> {
        let mut warnings = Vec::new();
        let Some(bus_address) = desktop.dbus_session_bus_address.as_ref() else {
            warnings.push("DBUS_SESSION_BUS_ADDRESS is not set; desktop portals disabled".into());
            return Ok(Self {
                proxy: None,
                mode: PortalMode::Disabled,
                warnings,
            });
        };

        let helper = ensure_bridge_helper(paths)?;
        let app_scope = app_scope_name(app_id);
        let shared_dir = shared_portal_dir(paths, app_id);
        let doc_dir = shared_dir.join("doc");
        let sandbox_doc_dir = sandbox_root
            .join("run")
            .join("user")
            .join(uid.to_string())
            .join("doc");

        let bus_dir = shared_dir.join("bus");
        fs::create_dir_all(paths.portal().join("locks")).context("create portal lock directory")?;
        let lock_path = paths
            .portal()
            .join("locks")
            .join(format!("{app_scope}.lock"));
        let lock = lock_portal_scope(&lock_path)?;
        fs::create_dir_all(&doc_dir).with_context(|| format!("create {}", doc_dir.display()))?;
        fs::create_dir_all(&bus_dir).with_context(|| format!("create {}", bus_dir.display()))?;
        fs::write(shared_dir.join("app-id"), app_id)
            .with_context(|| format!("write portal app scope for {app_id}"))?;
        let bus_socket = bus_dir.join("bus");
        let host_private_bus_address = format!("unix:path={}", bus_socket.display());
        let mountpoint = format!("/run/user/{uid}/doc");
        if !shared_portal_ready(&host_private_bus_address, &mountpoint) {
            stop_shared_portal(&shared_dir)?;
            fs::create_dir_all(&doc_dir)
                .with_context(|| format!("create {}", doc_dir.display()))?;
            fs::create_dir_all(&bus_dir)
                .with_context(|| format!("create {}", bus_dir.display()))?;
            fs::write(shared_dir.join("app-id"), app_id)
                .with_context(|| format!("write portal app scope for {app_id}"))?;
            let bus_config = bus_dir.join("session.conf");
            fs::write(&bus_config, private_bus_config(&bus_socket))
                .with_context(|| format!("write {}", bus_config.display()))?;

            let (mut bus_child, address) = start_private_bus(&bus_config)?;
            fs::write(shared_dir.join("bus.pid"), bus_child.id().to_string())
                .context("write private bus pid")?;
            let app_sandbox_root = paths.chroots().join(app_scope_name(app_id));
            let mut bridge_command = Command::new(&helper);
            bridge_command
                .arg("--app-id")
                .arg(app_id)
                .arg("--doc-dir")
                .arg(&doc_dir)
                .arg("--sandbox-root")
                .arg(&app_sandbox_root)
                .arg("--mountpoint")
                .arg(&mountpoint)
                .env("DBUS_SESSION_BUS_ADDRESS", &address)
                .env("HOST_DBUS_SESSION_BUS_ADDRESS", bus_address)
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
            detach_shared_process(&mut bridge_command);
            let mut bridge_child = bridge_command
                .spawn()
                .with_context(|| format!("start {}", helper.display()))?;
            fs::write(shared_dir.join("bridge.pid"), bridge_child.id().to_string())
                .context("write portal bridge pid")?;
            if let Err(error) = wait_for_portal_proxy(&address, &mountpoint) {
                terminate_child(&mut bridge_child);
                terminate_child(&mut bus_child);
                return Err(error).context("wait for shared document portal bridge");
            }
        }
        drop(lock);
        let sandbox_bus_address = sandbox_bus_address(&desktop.xdg_runtime_dir, &bus_socket, uid)
            .with_context(|| {
            format!(
                "map private bus {} into chroot /run/user/{uid}",
                bus_socket.display()
            )
        })?;

        Ok(Self {
            proxy: Some(PortalProxy {
                paths: paths.clone(),
                app_id: app_id.to_string(),
                instance_id: instance_id.to_string(),
                shared_dir,
                doc_dir,
                sandbox_doc_dir,
                private_bus_address: host_private_bus_address,
                sandbox_bus_address,
            }),
            mode: PortalMode::PrivateProxy,
            warnings,
        })
    }

    pub fn env(&self) -> Vec<(String, String)> {
        let mut env = vec![("GTK_USE_PORTAL".to_string(), "1".to_string())];
        if let Some(proxy) = &self.proxy {
            env.push((
                "DBUS_SESSION_BUS_ADDRESS".to_string(),
                proxy.sandbox_bus_address.clone(),
            ));
        }
        env
    }

    pub fn doc_dir(&self) -> Option<&Path> {
        self.proxy.as_ref().map(|proxy| proxy.doc_dir.as_path())
    }

    pub fn attach_sandbox(&self) -> Result<()> {
        let Some(proxy) = &self.proxy else {
            return Ok(());
        };
        portal_control(proxy, "AddSandbox")
    }

    pub fn describe(&self) -> Vec<String> {
        match (&self.mode, &self.proxy) {
            (PortalMode::PrivateProxy, Some(proxy)) => vec![
                format!(
                    "shared app bus: {}",
                    proxy
                        .sandbox_bus_address
                        .strip_prefix("unix:path=")
                        .unwrap_or(&proxy.sandbox_bus_address)
                ),
                format!(
                    "document grants: {} -> /run/user/*/doc",
                    proxy.doc_dir.display()
                ),
                format!(
                    "document mount targets: {}",
                    proxy.sandbox_doc_dir.display()
                ),
            ],
            (PortalMode::Disabled, _) => vec!["disabled".to_string()],
            (PortalMode::PrivateProxy, None) => vec!["private portal proxy stopped".to_string()],
        }
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn cleanup(&mut self) -> Result<()> {
        let Some(proxy) = self.proxy.as_mut() else {
            return Ok(());
        };

        portal_control(proxy, "RemoveSandbox")?;
        if !other_active_app_instances(&proxy.paths, &proxy.app_id, &proxy.instance_id)? {
            let app_scope = app_scope_name(&proxy.app_id);
            let lock_path = proxy
                .paths
                .portal()
                .join("locks")
                .join(format!("{app_scope}.lock"));
            let _lock = lock_portal_scope(&lock_path)?;
            if !other_active_app_instances(&proxy.paths, &proxy.app_id, &proxy.instance_id)? {
                stop_shared_portal(&proxy.shared_dir)?;
            }
        }
        self.proxy = None;
        Ok(())
    }
}

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

fn shared_portal_ready(bus_address: &str, mountpoint: &str) -> bool {
    bus_address
        .strip_prefix("unix:path=")
        .is_some_and(|path| Path::new(path).exists())
        && document_portal_ready(bus_address, mountpoint)
        && desktop_portal_ready(bus_address)
}

fn shared_portal_dir(paths: &Installation, app_id: &str) -> PathBuf {
    paths.portal().join("apps").join(app_scope_name(app_id))
}

fn app_scope_name(app_id: &str) -> String {
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

fn portal_control(proxy: &PortalProxy, method: &str) -> Result<()> {
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
        .arg(proxy.sandbox_doc_dir.display().to_string())
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

fn lock_portal_scope(path: &Path) -> Result<fs::File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let file = OpenOptions::new()
        .create(true)
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

fn stop_shared_portal(shared_dir: &Path) -> Result<()> {
    for name in ["bridge.pid", "bus.pid"] {
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

fn other_active_app_instances(
    paths: &Installation,
    app_id: &str,
    instance_id: &str,
) -> Result<bool> {
    app_has_active_run(paths, app_id, Some(instance_id))
}

fn app_has_active_run(
    paths: &Installation,
    app_id: &str,
    excluded_instance: Option<&str>,
) -> Result<bool> {
    for record in crate::state::read_run_records(paths)? {
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

fn ensure_bridge_helper(paths: &Installation) -> Result<PathBuf> {
    let output = paths.libexec_root().join("portal-bridge");
    if !output.is_file() {
        bail!("installed portal helper is missing: {}", output.display());
    }
    Ok(output)
}

fn start_private_bus(config: &Path) -> Result<(Child, String)> {
    let mut command = Command::new("dbus-daemon");
    command
        .arg("--nofork")
        .arg("--print-address=1")
        .arg(format!("--config-file={}", config.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    detach_shared_process(&mut command);
    let mut child = command
        .spawn()
        .context("start private portal dbus-daemon")?;

    let stdout = child
        .stdout
        .take()
        .context("private dbus-daemon stdout was not captured")?;
    let mut reader = BufReader::new(stdout);
    let mut address = String::new();
    reader
        .read_line(&mut address)
        .context("read private dbus-daemon address")?;
    let address = address.trim().to_string();
    if !address.starts_with("unix:path=") {
        terminate_child(&mut child);
        bail!("private dbus-daemon did not print a unix:path address: {address}");
    }
    Ok((child, address))
}

fn detach_shared_process(command: &mut Command) {
    // The bus and bridge serve every live sandbox for an app. A new session
    // keeps terminal signals sent to whichever `flatpak run` created them
    // from killing app-scoped infrastructure used by the other runners.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

fn private_bus_config(socket: &Path) -> String {
    format!(
        r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow user="*"/>
    <allow own="*"/>
    <allow send_destination="*"/>
    <allow eavesdrop="true"/>
    <allow send_type="method_call"/>
    <allow send_type="method_return"/>
    <allow send_type="signal"/>
    <allow send_type="error"/>
    <allow send_requested_reply="true" send_type="method_return"/>
    <allow send_requested_reply="true" send_type="error"/>
    <allow send_requested_reply="false" send_type="method_return"/>
    <allow send_requested_reply="false" send_type="error"/>
    <allow receive_requested_reply="true" receive_type="method_return"/>
    <allow receive_requested_reply="true" receive_type="error"/>
    <allow receive_requested_reply="false" receive_type="method_return"/>
    <allow receive_requested_reply="false" receive_type="error"/>
    <allow receive_type="method_call"/>
    <allow receive_type="method_return"/>
    <allow receive_type="signal"/>
    <allow receive_type="error"/>
  </policy>
</busconfig>
"#,
        xml_escape(&socket.display().to_string())
    )
}

fn xml_escape(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn sandbox_bus_address(xdg_runtime_dir: &Path, bus_socket: &Path, uid: u32) -> Result<String> {
    let relative = bus_socket.strip_prefix(xdg_runtime_dir).with_context(|| {
        format!(
            "{} is not under XDG_RUNTIME_DIR {}",
            bus_socket.display(),
            xdg_runtime_dir.display()
        )
    })?;
    Ok(format!("unix:path=/run/user/{uid}/{}", relative.display()))
}

fn wait_for_portal_proxy(bus_address: &str, mountpoint: &str) -> Result<()> {
    for _ in 0..40 {
        if document_portal_ready(bus_address, mountpoint) && desktop_portal_ready(bus_address) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    bail!(
        "portal proxy did not publish FileChooser, ScreenCast, and document mountpoint {mountpoint}"
    );
}

fn document_portal_ready(bus_address: &str, mountpoint: &str) -> bool {
    let output = Command::new("gdbus")
        .arg("call")
        .arg("--session")
        .arg("--dest")
        .arg("org.freedesktop.portal.Desktop")
        .arg("--object-path")
        .arg("/org/freedesktop/portal/documents")
        .arg("--method")
        .arg("org.freedesktop.portal.Documents.GetMountPoint")
        .env("DBUS_SESSION_BUS_ADDRESS", bus_address)
        .output();

    matches!(output, Ok(output) if output.status.success()
        && String::from_utf8_lossy(&output.stdout).contains(mountpoint))
}

fn desktop_portal_ready(bus_address: &str) -> bool {
    portal_property_ready(bus_address, "org.freedesktop.portal.FileChooser", "version")
        && portal_property_ready(
            bus_address,
            "org.freedesktop.portal.ScreenCast",
            "AvailableSourceTypes",
        )
}

fn portal_property_ready(bus_address: &str, interface: &str, property: &str) -> bool {
    let output = Command::new("gdbus")
        .arg("call")
        .arg("--session")
        .arg("--dest")
        .arg("org.freedesktop.portal.Desktop")
        .arg("--object-path")
        .arg("/org/freedesktop/portal/desktop")
        .arg("--method")
        .arg("org.freedesktop.DBus.Properties.Get")
        .arg(interface)
        .arg(property)
        .env("DBUS_SESSION_BUS_ADDRESS", bus_address)
        .output();

    matches!(output, Ok(output) if output.status.success())
}

fn terminate_child(child: &mut Child) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }

    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    for _ in 0..20 {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn unmount_under(root: &Path) -> Result<()> {
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

fn remove_dir(path: &Path) -> Result<()> {
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
    for record in crate::state::read_run_records(paths)? {
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

fn process_alive(pid: i32) -> bool {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use glib::variant::ToVariant;

    fn test_paths(name: &str) -> Installation {
        let root = std::env::temp_dir().join(format!(
            "freebsd-flatpak-portal-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        Installation::for_test(&root)
    }

    #[test]
    fn same_app_instances_share_one_dbus_scope() {
        let paths = test_paths("shared-dbus-scope");

        let first = shared_portal_dir(&paths, "org.example.App").join("bus/bus");
        let second = shared_portal_dir(&paths, "org.example.App").join("bus/bus");
        let other = shared_portal_dir(&paths, "org.example.Other").join("bus/bus");

        assert_eq!(first, second);
        assert_ne!(first, other);
    }

    #[test]
    fn shared_portal_survives_either_non_final_instance_and_stops_after_last() {
        let paths = test_paths("shared-lifetime");
        let first_root = paths.chroots().join("org.example.App/first");
        let second_root = paths.chroots().join("org.example.App/second");
        let other_root = paths.chroots().join("org.example.Other/only");
        let first_record = crate::state::write_run_record(
            &paths,
            "org.example.App",
            "first",
            &first_root,
            std::process::id(),
            0,
        )
        .unwrap();
        crate::state::write_run_record(
            &paths,
            "org.example.Other",
            "only",
            &other_root,
            std::process::id(),
            0,
        )
        .unwrap();
        let second_record = crate::state::write_run_record(
            &paths,
            "org.example.App",
            "second",
            &second_root,
            std::process::id(),
            0,
        )
        .unwrap();

        assert!(other_active_app_instances(&paths, "org.example.App", "first").unwrap());
        assert!(other_active_app_instances(&paths, "org.example.App", "second").unwrap());

        crate::state::remove_run_record(&first_record).unwrap();
        assert!(!other_active_app_instances(&paths, "org.example.App", "second").unwrap());
        crate::state::write_run_record(
            &paths,
            "org.example.App",
            "first",
            &first_root,
            std::process::id(),
            0,
        )
        .unwrap();
        crate::state::remove_run_record(&second_record).unwrap();
        assert!(!other_active_app_instances(&paths, "org.example.App", "first").unwrap());
    }

    #[test]
    fn shared_portal_process_is_detached_from_the_creating_runner_session() {
        let mut command = Command::new("sleep");
        command.arg("30").stdin(Stdio::null());
        detach_shared_process(&mut command);
        let mut child = command.spawn().unwrap();
        let child_pid = child.id() as i32;

        assert_eq!(unsafe { libc::getsid(child_pid) }, child_pid);

        terminate_child(&mut child);
    }

    #[test]
    fn two_connections_on_shared_bus_observe_one_name_owner() {
        let bus_dir = std::env::temp_dir().join(format!("ffp-dbus-{}", std::process::id()));
        let _ = fs::remove_dir_all(&bus_dir);
        fs::create_dir_all(&bus_dir).unwrap();
        let socket = bus_dir.join("bus");
        let config = bus_dir.join("session.conf");
        fs::write(&config, private_bus_config(&socket)).unwrap();
        let (mut child, address) = start_private_bus(&config).unwrap();
        let flags = gio::DBusConnectionFlags::AUTHENTICATION_CLIENT
            | gio::DBusConnectionFlags::MESSAGE_BUS_CONNECTION;
        let first =
            gio::DBusConnection::for_address_sync(&address, flags, None, gio::Cancellable::NONE)
                .unwrap();
        let second =
            gio::DBusConnection::for_address_sync(&address, flags, None, gio::Cancellable::NONE)
                .unwrap();

        let requested = first
            .call_sync(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "RequestName",
                Some(&("org.example.App.Remote", 4u32).to_variant()),
                Some(glib::VariantTy::new("(u)").unwrap()),
                gio::DBusCallFlags::NONE,
                -1,
                gio::Cancellable::NONE,
            )
            .unwrap();
        assert_eq!(requested.get::<(u32,)>().unwrap().0, 1);

        let visible = second
            .call_sync(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "NameHasOwner",
                Some(&("org.example.App.Remote",).to_variant()),
                Some(glib::VariantTy::new("(b)").unwrap()),
                gio::DBusCallFlags::NONE,
                -1,
                gio::Cancellable::NONE,
            )
            .unwrap();
        assert!(visible.get::<(bool,)>().unwrap().0);

        drop(second);
        drop(first);
        terminate_child(&mut child);
        let _ = fs::remove_dir_all(&bus_dir);
    }
}
