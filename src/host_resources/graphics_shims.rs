use super::drm_device::DrmDevice;
use crate::installation::installation_paths::Installation;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub(super) struct WaylandDrmDevtShim {
    pub(super) host_dir: PathBuf,
    pub(super) dev_t_map: String,
}

#[derive(Debug, Clone)]
pub(super) struct DrmSyncobjErrnoShim {
    pub(super) host_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub(super) struct Gtk3WaylandGeometryShim {
    pub(super) host_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub(super) struct ChromiumZygoteDrmPreload {
    pub(super) host_dir: PathBuf,
}
impl WaylandDrmDevtShim {
    pub(super) fn prepare(paths: &Installation, device: &DrmDevice) -> Result<Self> {
        let helper = ensure_wayland_drm_devt_shim(paths)?;
        let host_dir = helper
            .parent()
            .context("Wayland DRM dev_t shim output path has no parent")?
            .to_path_buf();
        Ok(Self {
            host_dir,
            dev_t_map: device.dev_t_map(),
        })
    }
}

impl DrmSyncobjErrnoShim {
    pub(super) fn prepare(paths: &Installation) -> Result<Self> {
        let helper = ensure_drm_syncobj_errno_shim(paths)?;
        let host_dir = helper
            .parent()
            .context("DRM syncobj errno shim output path has no parent")?
            .to_path_buf();
        Ok(Self { host_dir })
    }
}

impl Gtk3WaylandGeometryShim {
    pub(super) fn prepare(paths: &Installation) -> Result<Self> {
        let helper = ensure_gtk3_wayland_geometry_shim(paths)?;
        let host_dir = helper
            .parent()
            .context("GTK3 Wayland geometry shim output path has no parent")?
            .to_path_buf();
        Ok(Self { host_dir })
    }
}

impl ChromiumZygoteDrmPreload {
    pub(super) fn prepare(paths: &Installation) -> Result<Self> {
        let helper = ensure_chromium_zygote_drm_preload(paths)?;
        let host_dir = helper
            .parent()
            .context("Chromium zygote DRM preload output path has no parent")?
            .to_path_buf();
        Ok(Self { host_dir })
    }
}

fn ensure_wayland_drm_devt_shim(paths: &Installation) -> Result<PathBuf> {
    installed_helper(paths, "libwayland-drm-devt-shim.so")
}

fn ensure_drm_syncobj_errno_shim(paths: &Installation) -> Result<PathBuf> {
    installed_helper(paths, "libdrm-syncobj-errno-shim.so")
}

fn ensure_gtk3_wayland_geometry_shim(paths: &Installation) -> Result<PathBuf> {
    installed_helper(paths, "libgtk3-wayland-geometry-shim.so")
}

fn ensure_chromium_zygote_drm_preload(paths: &Installation) -> Result<PathBuf> {
    installed_helper(paths, "libchromium-zygote-drm-preload.so")
}

fn installed_helper(paths: &Installation, name: &str) -> Result<PathBuf> {
    let output = paths.libexec_root().join(name);
    if !output.is_file() {
        bail!("installed graphics helper is missing: {}", output.display());
    }
    Ok(output)
}

#[cfg(test)]
#[path = "tests/graphics_shims.rs"]
mod tests;
