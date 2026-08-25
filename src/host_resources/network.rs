use crate::installation::installation_paths::Installation;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

const NETLINK_ROUTE_FLAGS_SHIM_LIB: &str = "libnetlink-route-flags-shim.so";
const HELPER_SANDBOX_DIR: &str = "/run/host/freebsd-flatpak";

#[derive(Debug, Clone)]
pub struct HostNetwork {
    helper_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct NetworkMount {
    host_path: PathBuf,
    sandbox_path: PathBuf,
}

impl HostNetwork {
    pub fn prepare(paths: &Installation, network_enabled: bool) -> Result<Self> {
        if !network_enabled {
            return Ok(Self { helper_dir: None });
        }
        let helper = paths.libexec_root().join(NETLINK_ROUTE_FLAGS_SHIM_LIB);
        if !helper.is_file() {
            bail!("installed network helper is missing: {}", helper.display());
        }
        let helper_dir = helper
            .parent()
            .context("network helper path has no parent")?
            .to_path_buf();
        Ok(Self {
            helper_dir: Some(helper_dir),
        })
    }

    pub fn runtime_mount(&self) -> Option<NetworkMount> {
        self.helper_dir.as_ref().map(|host_path| NetworkMount {
            host_path: host_path.clone(),
            sandbox_path: PathBuf::from(HELPER_SANDBOX_DIR),
        })
    }

    pub fn preload_paths(&self) -> Vec<String> {
        self.helper_dir
            .iter()
            .map(|_| format!("{HELPER_SANDBOX_DIR}/{NETLINK_ROUTE_FLAGS_SHIM_LIB}"))
            .collect()
    }
}

impl NetworkMount {
    pub fn host_path(&self) -> &Path {
        &self.host_path
    }

    pub fn sandbox_target_relative(&self) -> Result<PathBuf> {
        self.sandbox_path
            .strip_prefix("/")
            .map(Path::to_path_buf)
            .context("network mount target must be absolute")
    }
}

#[cfg(test)]
#[path = "tests/network.rs"]
mod tests;
