use crate::installation::installation_paths::Installation;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;

const SIGNALFD_SHIM_LIB: &str = "libsignalfd-shim.so";
const FLATPAK_SPAWN_WRAPPER: &str = "linux-bin/flatpak-spawn";
const HELPER_SANDBOX_DIR: &str = "/run/host/freebsd-flatpak";
const HELPER_BIN_SANDBOX_DIR: &str = "/run/host/freebsd-flatpak/linux-bin";

#[derive(Debug, Clone)]
pub struct HostLinuxCompat {
    helper_dir: PathBuf,
}

impl HostLinuxCompat {
    pub fn prepare(paths: &Installation) -> Result<Self> {
        let helper = paths.libexec_root().join(SIGNALFD_SHIM_LIB);
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
        let helper_dir = helper
            .parent()
            .context("Linux compatibility helper path has no parent")?
            .to_path_buf();
        Ok(Self { helper_dir })
    }

    pub fn runtime_mount(&self) -> (PathBuf, PathBuf) {
        (
            self.helper_dir.clone(),
            PathBuf::from(HELPER_SANDBOX_DIR.trim_start_matches('/')),
        )
    }

    pub fn path_entries(&self) -> Vec<String> {
        vec![HELPER_BIN_SANDBOX_DIR.to_string()]
    }
}

#[cfg(test)]
#[path = "tests/linux_compat.rs"]
mod tests;
