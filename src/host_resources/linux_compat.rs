use crate::installation::installation_paths::Installation;
use anyhow::{bail, Context, Result};
use std::fs;
use std::os::unix::fs as unix_fs;
use std::path::{Path, PathBuf};

const SIGNALFD_SHIM_LIB: &str = "libsignalfd-shim.so";
const SOCKET_OPTION_ERRNO_SHIM_LIB: &str = "libsocket-option-errno-shim.so";
const FLATPAK_SPAWN_WRAPPER: &str = "linux-bin/flatpak-spawn";
const HELPER_SANDBOX_DIR: &str = "/run/host/freebsd-flatpak";
const RUNTIME_BIN: &str = "bin";
const RUNTIME_FLATPAK_SPAWN: &str = "flatpak-spawn";
const RUNTIME_BIN_SANDBOX_PATH: &str = "/run/freebsd-flatpak/runtime-bin";
const BIN_OVERLAY_RELATIVE: &str = ".freebsd-flatpak-linux-compat/bin";
const SESSION_BUS_RELATIVE: &str = "run/freebsd-flatpak/session-bus";
const SESSION_BUS_ADDRESS_PREFIX: &str = "unix:path=";
const BIN_SANDBOX_PATH: &str = "/usr/bin";
const FLATPAK_SPAWN_SANDBOX_PATH: &str = "/usr/bin/flatpak-spawn";

#[derive(Debug, Clone)]
pub struct HostLinuxCompat {
    helper_dir: PathBuf,
    spawn_wrapper: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxCompatMount {
    source: PathBuf,
    sandbox_path: PathBuf,
}

impl HostLinuxCompat {
    pub fn prepare(paths: &Installation) -> Result<Self> {
        let helper = paths.libexec_root().join(SIGNALFD_SHIM_LIB);
        let socket_option_helper = paths.libexec_root().join(SOCKET_OPTION_ERRNO_SHIM_LIB);
        let spawn_wrapper = paths.libexec_root().join(FLATPAK_SPAWN_WRAPPER);
        if !spawn_wrapper.is_file() {
            bail!(
                "installed flatpak-spawn compatibility wrapper is missing: {}",
                spawn_wrapper.display()
            );
        }
        if !helper.is_file() {
            bail!(
                "installed Linux compatibility helper is missing: {}",
                helper.display()
            );
        }
        if !socket_option_helper.is_file() {
            bail!(
                "installed Linux socket-option compatibility helper is missing: {}",
                socket_option_helper.display()
            );
        }
        let helper_dir = helper
            .parent()
            .context("Linux compatibility helper path has no parent")?
            .to_path_buf();
        Ok(Self {
            helper_dir,
            spawn_wrapper,
        })
    }

    pub fn runtime_mount(&self) -> (PathBuf, PathBuf) {
        (
            self.helper_dir.clone(),
            PathBuf::from(HELPER_SANDBOX_DIR.trim_start_matches('/')),
        )
    }

    pub fn preload_paths(&self) -> Vec<String> {
        vec![format!(
            "{HELPER_SANDBOX_DIR}/{SOCKET_OPTION_ERRNO_SHIM_LIB}"
        )]
    }

    pub fn prepare_runtime_binary_mounts(
        &self,
        sandbox_root: &Path,
        runtime_files: &Path,
        sandbox_bus_address: Option<&str>,
    ) -> Result<Vec<LinuxCompatMount>> {
        let runtime_bin = runtime_files.join(RUNTIME_BIN);
        if !runtime_bin.join(RUNTIME_FLATPAK_SPAWN).is_file() {
            return Ok(Vec::new());
        }

        if let Some(bus_path) =
            sandbox_bus_address.and_then(|address| address.strip_prefix(SESSION_BUS_ADDRESS_PREFIX))
        {
            let bus_path = Path::new(bus_path);
            if !bus_path.is_absolute() {
                bail!("sandbox session bus path must be absolute");
            }
            let session_bus = sandbox_root.join(SESSION_BUS_RELATIVE);
            let parent = session_bus
                .parent()
                .context("Linux compatibility session bus path has no parent")?;
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "create Linux compatibility runtime directory {}",
                    parent.display()
                )
            })?;
            unix_fs::symlink(bus_path, &session_bus)
                .with_context(|| format!("link private session bus {}", session_bus.display()))?;
        }

        let overlay_bin = sandbox_root.join(BIN_OVERLAY_RELATIVE);
        fs::create_dir_all(&overlay_bin).with_context(|| {
            format!(
                "create Linux compatibility bin overlay {}",
                overlay_bin.display()
            )
        })?;
        for entry in fs::read_dir(&runtime_bin)
            .with_context(|| format!("read runtime bin directory {}", runtime_bin.display()))?
        {
            let entry = entry.with_context(|| {
                format!(
                    "read entry in runtime bin directory {}",
                    runtime_bin.display()
                )
            })?;
            let name = entry.file_name();
            let overlay_entry = overlay_bin.join(&name);
            if name == RUNTIME_FLATPAK_SPAWN {
                fs::write(&overlay_entry, []).with_context(|| {
                    format!(
                        "create flatpak-spawn compatibility mountpoint {}",
                        overlay_entry.display()
                    )
                })?;
            } else {
                unix_fs::symlink(
                    Path::new(RUNTIME_BIN_SANDBOX_PATH).join(&name),
                    &overlay_entry,
                )
                .with_context(|| format!("link runtime binary {}", overlay_entry.display()))?;
            }
        }

        Ok(vec![
            LinuxCompatMount {
                source: runtime_bin,
                sandbox_path: PathBuf::from(RUNTIME_BIN_SANDBOX_PATH),
            },
            LinuxCompatMount {
                source: overlay_bin,
                sandbox_path: PathBuf::from(BIN_SANDBOX_PATH),
            },
            LinuxCompatMount {
                source: self.spawn_wrapper.clone(),
                sandbox_path: PathBuf::from(FLATPAK_SPAWN_SANDBOX_PATH),
            },
        ])
    }
}

impl LinuxCompatMount {
    pub fn host_path(&self) -> &Path {
        &self.source
    }

    pub fn sandbox_target_relative(&self) -> Result<PathBuf> {
        self.sandbox_path
            .strip_prefix("/")
            .map(Path::to_path_buf)
            .context("Linux compatibility mount target must be absolute")
    }
}

#[cfg(test)]
#[path = "tests/linux_compat.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/linux_socket_compat.rs"]
mod socket_tests;
