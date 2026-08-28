use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

pub(super) fn shared_portal_ready(
    bus_address: &str,
    mountpoint: &str,
    expect_host_command: bool,
) -> bool {
    bus_address
        .strip_prefix("unix:path=")
        .is_some_and(|path| Path::new(path).exists())
        && document_portal_ready(bus_address, mountpoint)
        && desktop_portal_ready(bus_address)
        && status_notifier_ready(bus_address)
        && flatpak_development_ready(bus_address) == expect_host_command
}

pub(super) fn start_private_bus(config: &Path) -> Result<(Child, String)> {
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

pub(super) fn detach_shared_process(command: &mut Command) {
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

pub(super) fn private_bus_config(socket: &Path) -> String {
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

pub(super) fn sandbox_bus_address(
    xdg_runtime_dir: &Path,
    bus_socket: &Path,
    uid: u32,
) -> Result<String> {
    let relative = bus_socket.strip_prefix(xdg_runtime_dir).with_context(|| {
        format!(
            "{} is not under XDG_RUNTIME_DIR {}",
            bus_socket.display(),
            xdg_runtime_dir.display()
        )
    })?;
    Ok(format!("unix:path=/run/user/{uid}/{}", relative.display()))
}

pub(super) fn wait_for_portal_proxy(
    bus_address: &str,
    mountpoint: &str,
    expect_host_command: bool,
) -> Result<()> {
    for _ in 0..40 {
        if document_portal_ready(bus_address, mountpoint)
            && desktop_portal_ready(bus_address)
            && status_notifier_ready(bus_address)
            && flatpak_development_ready(bus_address) == expect_host_command
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    bail!(
        "compatibility bridges did not publish the requested per-app interfaces and document mountpoint {mountpoint}"
    );
}

fn flatpak_development_ready(bus_address: &str) -> bool {
    let output = Command::new("gdbus")
        .arg("call")
        .arg("--session")
        .arg("--dest")
        .arg("org.freedesktop.DBus")
        .arg("--object-path")
        .arg("/org/freedesktop/DBus")
        .arg("--method")
        .arg("org.freedesktop.DBus.NameHasOwner")
        .arg("org.freedesktop.Flatpak")
        .env("DBUS_SESSION_BUS_ADDRESS", bus_address)
        .output();
    matches!(output, Ok(output) if output.status.success()
        && String::from_utf8_lossy(&output.stdout).contains("true"))
}

fn status_notifier_ready(bus_address: &str) -> bool {
    let output = Command::new("gdbus")
        .arg("call")
        .arg("--session")
        .arg("--dest")
        .arg("org.freedesktop.DBus")
        .arg("--object-path")
        .arg("/org/freedesktop/DBus")
        .arg("--method")
        .arg("org.freedesktop.DBus.NameHasOwner")
        .arg("org.kde.StatusNotifierWatcher")
        .env("DBUS_SESSION_BUS_ADDRESS", bus_address)
        .output();
    matches!(output, Ok(output) if output.status.success()
        && String::from_utf8_lossy(&output.stdout).contains("true"))
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
        && portal_property_ready(bus_address, "org.freedesktop.portal.OpenURI", "version")
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

pub(crate) fn terminate_child(child: &mut Child) {
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

#[cfg(test)]
#[path = "tests/private_session_bus.rs"]
mod tests;
