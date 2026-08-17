use crate::desktop::DesktopSession;
use anyhow::{bail, Context, Result};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime};

const BRIDGE_SOURCE: &str = "scripts/portal-bridge.c";
const BRIDGE_BIN: &str = "target/portal/portal-bridge";

#[derive(Debug)]
pub struct HostPortal {
    proxy: Option<PortalProxy>,
    mode: PortalMode,
    warnings: Vec<String>,
}

#[derive(Debug)]
struct PortalProxy {
    bus_child: Child,
    bridge_child: Child,
    bus_dir: PathBuf,
    doc_dir: PathBuf,
    sandbox_doc_dir: PathBuf,
    sandbox_bus_address: String,
}

#[derive(Debug, Clone, Copy)]
enum PortalMode {
    PrivateProxy,
    Disabled,
}

impl HostPortal {
    pub fn prepare(
        project_root: &Path,
        app_id: &str,
        desktop: &DesktopSession,
        uid: u32,
        sandbox_root: &Path,
    ) -> Result<Self> {
        let mut warnings = Vec::new();
        let Some(bus_address) = desktop.dbus_session_bus_address.as_ref() else {
            warnings
                .push("DBUS_SESSION_BUS_ADDRESS is not set; FileChooser portals disabled".into());
            return Ok(Self {
                proxy: None,
                mode: PortalMode::Disabled,
                warnings,
            });
        };

        let helper = ensure_bridge_helper(project_root)?;
        let run_id = format!("{}-{}", sanitize_id(app_id), std::process::id());
        let doc_dir = project_root
            .join("runtime")
            .join("portal")
            .join("doc")
            .join(&run_id);
        fs::create_dir_all(&doc_dir).with_context(|| format!("create {}", doc_dir.display()))?;
        let sandbox_doc_dir = sandbox_root
            .join("run")
            .join("user")
            .join(uid.to_string())
            .join("doc");

        let bus_dir = desktop
            .xdg_runtime_dir
            .join("freebsd-flatpak-poc")
            .join(&run_id);
        fs::create_dir_all(&bus_dir).with_context(|| format!("create {}", bus_dir.display()))?;
        let bus_socket = bus_dir.join("bus");
        if bus_socket.exists() {
            fs::remove_file(&bus_socket)
                .with_context(|| format!("remove stale {}", bus_socket.display()))?;
        }

        let bus_config = bus_dir.join("session.conf");
        fs::write(&bus_config, private_bus_config(&bus_socket))
            .with_context(|| format!("write {}", bus_config.display()))?;

        let (mut bus_child, host_private_bus_address) = start_private_bus(&bus_config)?;
        let sandbox_bus_address = sandbox_bus_address(&desktop.xdg_runtime_dir, &bus_socket, uid)
            .with_context(|| {
            format!(
                "map private bus {} into chroot /run/user/{uid}",
                bus_socket.display()
            )
        })?;

        let mountpoint = format!("/run/user/{uid}/doc");
        let mut bridge_child = Command::new(&helper)
            .arg("--app-id")
            .arg(app_id)
            .arg("--doc-dir")
            .arg(&doc_dir)
            .arg("--sandbox-doc-dir")
            .arg(&sandbox_doc_dir)
            .arg("--mountpoint")
            .arg(&mountpoint)
            .env("DBUS_SESSION_BUS_ADDRESS", &host_private_bus_address)
            .env("HOST_DBUS_SESSION_BUS_ADDRESS", bus_address)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("start {}", helper.display()))?;

        if let Err(error) = wait_for_portal_proxy(&host_private_bus_address, &mountpoint) {
            terminate_child(&mut bridge_child);
            terminate_child(&mut bus_child);
            return Err(error).context("wait for document portal bridge");
        }

        Ok(Self {
            proxy: Some(PortalProxy {
                bus_child,
                bridge_child,
                bus_dir,
                doc_dir,
                sandbox_doc_dir,
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

    pub fn describe(&self) -> Vec<String> {
        match (&self.mode, &self.proxy) {
            (PortalMode::PrivateProxy, Some(proxy)) => vec![
                format!(
                    "private bus: {}",
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

        terminate_child(&mut proxy.bridge_child);
        unmount_nested_under(&proxy.sandbox_doc_dir)?;
        unmount_under(&proxy.doc_dir)?;
        remove_doc_dir(&proxy.doc_dir)?;
        terminate_child(&mut proxy.bus_child);
        remove_dir(&proxy.bus_dir)?;
        self.proxy = None;
        Ok(())
    }
}

pub fn recover_stale_portal_mounts(project_root: &Path) -> Result<()> {
    let active_launcher_pids = active_launcher_pids(project_root)?;
    let doc_root = project_root.join("runtime").join("portal").join("doc");
    unmount_under(&doc_root)?;
    if doc_root.is_dir() {
        for entry in fs::read_dir(&doc_root)
            .with_context(|| format!("read portal document root {}", doc_root.display()))?
        {
            let path = entry?.path();
            if path.is_dir() && !belongs_to_active_launcher(&path, &active_launcher_pids) {
                kill_processes_referencing(&path)?;
                remove_doc_dir(&path)?;
            }
        }
    }

    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
        let bus_root = runtime_dir.join("freebsd-flatpak-poc");
        if bus_root.is_dir() {
            for entry in fs::read_dir(&bus_root)
                .with_context(|| format!("read portal bus root {}", bus_root.display()))?
            {
                let path = entry?.path();
                if path.is_dir() && !belongs_to_active_launcher(&path, &active_launcher_pids) {
                    kill_processes_referencing(&path)?;
                    remove_dir(&path)?;
                }
            }
        }
    }
    Ok(())
}

fn ensure_bridge_helper(project_root: &Path) -> Result<PathBuf> {
    let source = project_root.join(BRIDGE_SOURCE);
    let output = project_root.join(BRIDGE_BIN);
    let output_dir = output
        .parent()
        .context("document portal bridge output path has no parent")?;
    fs::create_dir_all(output_dir).with_context(|| format!("create {}", output_dir.display()))?;

    if !needs_rebuild(&source, &output)? {
        return Ok(output);
    }

    let pkg_config = Command::new("pkg-config")
        .args(["--cflags", "--libs", "gio-2.0", "gio-unix-2.0", "glib-2.0"])
        .output()
        .context("run pkg-config for portal bridge")?;
    if !pkg_config.status.success() {
        bail!(
            "pkg-config failed for portal bridge with status {}",
            pkg_config.status
        );
    }
    let flags = String::from_utf8(pkg_config.stdout).context("pkg-config output is not UTF-8")?;

    let mut command = Command::new("cc");
    command.arg(&source).arg("-o").arg(&output);
    command.args(flags.split_whitespace());
    let status = command
        .status()
        .with_context(|| format!("compile {}", output.display()))?;
    if !status.success() {
        bail!("compile {} failed with status {}", output.display(), status);
    }
    Ok(output)
}

fn start_private_bus(config: &Path) -> Result<(Child, String)> {
    let mut child = Command::new("dbus-daemon")
        .arg("--nofork")
        .arg("--print-address=1")
        .arg(format!("--config-file={}", config.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
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

fn needs_rebuild(source: &Path, output: &Path) -> Result<bool> {
    let Ok(output_meta) = fs::metadata(output) else {
        return Ok(true);
    };
    let source_time = modified(source)?;
    let output_time = output_meta
        .modified()
        .with_context(|| format!("read mtime {}", output.display()))?;
    Ok(source_time > output_time)
}

fn modified(path: &Path) -> Result<SystemTime> {
    fs::metadata(path)
        .with_context(|| format!("stat {}", path.display()))?
        .modified()
        .with_context(|| format!("read mtime {}", path.display()))
}

fn wait_for_portal_proxy(bus_address: &str, mountpoint: &str) -> Result<()> {
    for _ in 0..40 {
        if document_portal_ready(bus_address, mountpoint) && desktop_portal_ready(bus_address) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    bail!("portal proxy did not publish FileChooser and document mountpoint {mountpoint}");
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
    let output = Command::new("gdbus")
        .arg("call")
        .arg("--session")
        .arg("--dest")
        .arg("org.freedesktop.portal.Desktop")
        .arg("--object-path")
        .arg("/org/freedesktop/portal/desktop")
        .arg("--method")
        .arg("org.freedesktop.DBus.Properties.Get")
        .arg("org.freedesktop.portal.FileChooser")
        .arg("version")
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
    if !root.exists() {
        return Ok(());
    }

    let mut mountpoints = mount_points_under(root)?;
    mountpoints.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for mountpoint in mountpoints {
        if let Err(error) = unmount_one(&mountpoint, false) {
            eprintln!(
                "warning: portal umount failed for {}: {error:#}",
                mountpoint.display()
            );
            unmount_one(&mountpoint, true)?;
        }
    }
    Ok(())
}

fn unmount_nested_under(root: &Path) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }

    let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut mountpoints = mount_points_under(&root)?;
    mountpoints.retain(|mountpoint| mountpoint != &root);
    mountpoints.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for mountpoint in mountpoints {
        if let Err(error) = unmount_one(&mountpoint, false) {
            eprintln!(
                "warning: portal grant umount failed for {}: {error:#}",
                mountpoint.display()
            );
            unmount_one(&mountpoint, true)?;
        }
    }
    Ok(())
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

fn active_launcher_pids(project_root: &Path) -> Result<Vec<i32>> {
    let mut pids = Vec::new();
    for record in crate::state::read_run_records(project_root)? {
        let pid = record
            .get("launcher_pid")
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(0);
        if pid > 0 && process_alive(pid) {
            pids.push(pid);
        }
    }
    Ok(pids)
}

fn belongs_to_active_launcher(path: &Path, active_launcher_pids: &[i32]) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    active_launcher_pids
        .iter()
        .any(|pid| name.ends_with(&format!("-{pid}")))
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
