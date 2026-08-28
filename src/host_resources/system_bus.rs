use crate::flatpak_metadata::section_entries;
use crate::installation::installation_paths::Installation;
use crate::portal_integration::terminate_child;
use anyhow::{bail, Context, Result};
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

const NETWORK_MANAGER_NAME: &str = "org.freedesktop.NetworkManager";
const NETWORK_MANAGER_HELPER: &str = "network-manager-compat";

#[derive(Debug)]
pub struct HostSystemBus {
    directory: Option<PathBuf>,
    bus: Option<Child>,
    service: Option<Child>,
    allowed_names: Vec<String>,
}

impl HostSystemBus {
    pub fn prepare(paths: &Installation, metadata: &str, instance_id: &str) -> Result<Self> {
        let allowed_names = system_talk_names(metadata);
        if allowed_names.is_empty() {
            return Ok(Self {
                directory: None,
                bus: None,
                service: None,
                allowed_names,
            });
        }

        let directory = paths.system_bus().join(compact_scope(instance_id));
        fs::create_dir_all(&directory)
            .with_context(|| format!("create private system bus {}", directory.display()))?;
        let socket = directory.join("system_bus_socket");
        let config = directory.join("system.conf");
        fs::write(&config, private_system_bus_config(&socket, &allowed_names))
            .with_context(|| format!("write {}", config.display()))?;
        let (bus, address) = match start_bus(&config) {
            Ok(started) => started,
            Err(error) => {
                if let Err(cleanup_error) = fs::remove_dir_all(&directory) {
                    return Err(error).context(format!(
                        "remove failed private system bus {}: {cleanup_error}",
                        directory.display()
                    ));
                }
                return Err(error);
            }
        };

        let mut result = Self {
            directory: Some(directory),
            bus: Some(bus),
            service: None,
            allowed_names,
        };
        if result
            .allowed_names
            .iter()
            .any(|name| name == NETWORK_MANAGER_NAME)
        {
            if let Err(error) = result.start_network_manager(paths, &address) {
                let _ = result.cleanup();
                return Err(error);
            }
        }
        Ok(result)
    }

    fn start_network_manager(&mut self, paths: &Installation, address: &str) -> Result<()> {
        let helper = paths.libexec_root().join(NETWORK_MANAGER_HELPER);
        if !helper.is_file() {
            bail!(
                "installed NetworkManager compatibility helper is missing: {}",
                helper.display()
            );
        }
        let mut command = Command::new(&helper);
        command
            .arg("--address")
            .arg(address)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        detach_owned_process(&mut command);
        self.service = Some(
            command
                .spawn()
                .with_context(|| format!("start {}", helper.display()))?,
        );
        wait_for_name(address, NETWORK_MANAGER_NAME)
    }

    pub fn runtime_mount(&self) -> Option<(PathBuf, PathBuf)> {
        self.directory
            .as_ref()
            .map(|directory| (directory.clone(), PathBuf::from("run/dbus")))
    }

    pub fn describe(&self) -> Vec<String> {
        self.directory
            .as_ref()
            .map(|_| {
                vec![format!(
                    "private system bus (allowed destinations: {})",
                    self.allowed_names.join(", ")
                )]
            })
            .unwrap_or_default()
    }

    pub fn cleanup(&mut self) -> Result<()> {
        if let Some(service) = self.service.as_mut() {
            terminate_child(service);
        }
        if let Some(bus) = self.bus.as_mut() {
            terminate_child(bus);
        }
        self.service = None;
        self.bus = None;
        if let Some(directory) = self.directory.take() {
            if directory.exists() {
                fs::remove_dir_all(&directory)
                    .with_context(|| format!("remove {}", directory.display()))?;
            }
        }
        Ok(())
    }
}

impl Drop for HostSystemBus {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            eprintln!("warning: private system bus cleanup failed: {error:#}");
        }
    }
}

fn detach_owned_process(command: &mut Command) {
    // Keep terminal signals aimed at the launched app away from this
    // per-instance infrastructure, but do not leave it behind if the launcher
    // crashes before normal cleanup.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            let mut signal = libc::SIGTERM;
            if libc::procctl(
                libc::P_PID,
                0,
                libc::PROC_PDEATHSIG_CTL,
                (&mut signal as *mut libc::c_int).cast(),
            ) == -1
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

fn start_bus(config: &Path) -> Result<(Child, String)> {
    let mut command = Command::new("dbus-daemon");
    command
        .arg("--nofork")
        .arg("--print-address=1")
        .arg(format!("--config-file={}", config.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    detach_owned_process(&mut command);
    let mut child = command
        .spawn()
        .context("start private system dbus-daemon")?;
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            terminate_child(&mut child);
            bail!("private system dbus-daemon stdout was not captured");
        }
    };
    let mut address = String::new();
    if let Err(error) = BufReader::new(stdout).read_line(&mut address) {
        terminate_child(&mut child);
        return Err(error).context("read private system bus address");
    }
    let address = address.trim().to_string();
    if !address.starts_with("unix:path=") {
        terminate_child(&mut child);
        bail!("private system dbus-daemon printed invalid address: {address}");
    }
    Ok((child, address))
}

fn compact_scope(instance_id: &str) -> String {
    let mut hasher = DefaultHasher::new();
    instance_id.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn wait_for_name(address: &str, name: &str) -> Result<()> {
    for _ in 0..40 {
        let output = Command::new("gdbus")
            .args([
                "call",
                "--address",
                address,
                "--dest",
                "org.freedesktop.DBus",
                "--object-path",
                "/org/freedesktop/DBus",
                "--method",
                "org.freedesktop.DBus.NameHasOwner",
                name,
            ])
            .output();
        if matches!(output, Ok(output) if output.status.success()
            && String::from_utf8_lossy(&output.stdout).contains("true"))
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    bail!("private system bus service {name} did not become ready")
}

fn system_talk_names(metadata: &str) -> Vec<String> {
    let mut names = section_entries(metadata, "System Bus Policy")
        .into_iter()
        .filter_map(|(name, policy)| (policy == "talk").then_some(name))
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

fn private_system_bus_config(socket: &Path, allowed_names: &[String]) -> String {
    let destinations = allowed_names
        .iter()
        .map(|name| format!("    <allow send_destination=\"{}\"/>\n", xml_escape(name)))
        .collect::<String>();
    let own_network_manager = if allowed_names
        .iter()
        .any(|name| name == NETWORK_MANAGER_NAME)
    {
        "    <allow own=\"org.freedesktop.NetworkManager\"/>\n"
    } else {
        ""
    };
    format!(
        "<busconfig>\n  <type>system</type>\n  <listen>unix:path={}</listen>\n  <auth>EXTERNAL</auth>\n  <policy context=\"default\">\n    <deny own=\"*\"/>\n    <deny send_destination=\"*\"/>\n    <deny eavesdrop=\"true\"/>\n    <allow send_destination=\"org.freedesktop.DBus\"/>\n{}{}    <allow send_requested_reply=\"true\"/>\n    <allow receive_requested_reply=\"true\"/>\n    <allow receive_type=\"signal\"/>\n    <allow receive_type=\"method_call\"/>\n  </policy>\n</busconfig>\n",
        xml_escape(&socket.display().to_string()), destinations, own_network_manager
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
#[path = "tests/system_bus.rs"]
mod tests;
