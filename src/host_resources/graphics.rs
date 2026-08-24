use super::drm_device::DrmDevice;
use super::graphics_shims::{ChromiumZygoteDrmPreload, DrmSyncobjErrnoShim, WaylandDrmDevtShim};
use super::linux_drm_sysfs::{linux_drm_dev_t, DrmSysfsBridge};
use crate::installation::installation_paths::Installation;
use crate::installation::{self as runtime, FlatpakApp, RuntimeGlExtension};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub(super) const DRM_MAJOR: u32 = 226;
pub(super) const DRM_SYNCOBJ_ERRNO_SHIM_LIB: &str = "libdrm-syncobj-errno-shim.so";
pub(super) const CHROMIUM_ZYGOTE_DRM_PRELOAD_LIB: &str = "libchromium-zygote-drm-preload.so";
pub(super) const GRAPHICS_SHIM_SANDBOX_DIR: &str = "/run/host/freebsd-flatpak";
pub(super) const WAYLAND_DRM_DEVT_SHIM_LIB: &str = "libwayland-drm-devt-shim.so";

#[derive(Debug, Clone)]
pub struct HostGraphics {
    gl: Option<RuntimeGlExtension>,
    drm: Option<DrmSysfsBridge>,
    drm_syncobj_errno_shim: Option<DrmSyncobjErrnoShim>,
    chromium_zygote_drm_preload: Option<ChromiumZygoteDrmPreload>,
    wayland_drm_devt_shim: Option<WaylandDrmDevtShim>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GraphicsMount {
    pub(super) host_path: PathBuf,
    pub(super) sandbox_path: PathBuf,
}

impl HostGraphics {
    pub fn prepare(paths: &Installation, app: &FlatpakApp, instance_id: &str) -> Result<Self> {
        let mut warnings = Vec::new();
        let gl = runtime::ensure_default_gl_extension(paths, &app.runtime_ref, &app.runtime_dir)?;
        let drm = if gl.is_some() {
            match DrmDevice::detect() {
                Ok(device) => Some(DrmSysfsBridge::prepare(
                    paths,
                    &app.app_id,
                    instance_id,
                    device,
                )?),
                Err(error) => {
                    warnings.push(format!("DRM sysfs bridge disabled: {error:#}"));
                    None
                }
            }
        } else {
            None
        };

        let wayland_drm_devt_shim = if let Some(drm) = &drm {
            match WaylandDrmDevtShim::prepare(paths, &drm.device) {
                Ok(shim) => Some(shim),
                Err(error) => {
                    warnings.push(format!("Wayland DRM dev_t shim disabled: {error:#}"));
                    None
                }
            }
        } else {
            None
        };

        let drm_syncobj_errno_shim = if drm.is_some() {
            match DrmSyncobjErrnoShim::prepare(paths) {
                Ok(shim) => Some(shim),
                Err(error) => {
                    warnings.push(format!("DRM syncobj errno shim disabled: {error:#}"));
                    None
                }
            }
        } else {
            None
        };

        let chromium_zygote_drm_preload = if drm_syncobj_errno_shim.is_some() {
            match ChromiumZygoteDrmPreload::prepare(paths) {
                Ok(shim) => Some(shim),
                Err(error) => {
                    warnings.push(format!("Chromium zygote DRM preload disabled: {error:#}"));
                    None
                }
            }
        } else {
            None
        };

        Ok(Self {
            gl,
            drm,
            drm_syncobj_errno_shim,
            chromium_zygote_drm_preload,
            wayland_drm_devt_shim,
            warnings,
        })
    }

    pub fn extension_refs(&self) -> impl Iterator<Item = &str> {
        self.gl.iter().map(|gl| gl.ref_name())
    }

    pub fn runtime_mounts(&self) -> Vec<GraphicsMount> {
        let mut mounts: Vec<GraphicsMount> = self
            .gl
            .iter()
            .map(|gl| GraphicsMount {
                host_path: gl.checkout_dir.join("files"),
                sandbox_path: PathBuf::from("/usr").join(&gl.runtime_mount_relative),
            })
            .collect();
        let shim_dir = self
            .drm_syncobj_errno_shim
            .as_ref()
            .map(|shim| &shim.host_dir)
            .or_else(|| {
                self.wayland_drm_devt_shim
                    .as_ref()
                    .map(|shim| &shim.host_dir)
            });
        if let Some(host_dir) = shim_dir {
            mounts.push(GraphicsMount {
                host_path: host_dir.clone(),
                sandbox_path: PathBuf::from(GRAPHICS_SHIM_SANDBOX_DIR),
            });
        }
        mounts
    }

    pub fn sysfs_mounts(&self) -> Vec<GraphicsMount> {
        self.drm
            .as_ref()
            .map(|drm| drm.mounts.clone())
            .unwrap_or_default()
    }

    pub fn env(&self) -> Vec<(String, String)> {
        if self.gl.is_none() {
            return Vec::new();
        }

        let gl_root = "/usr/lib/x86_64-linux-gnu/GL/default";
        let mut env = vec![
            (
                "LD_LIBRARY_PATH".to_string(),
                format!(
                    "{gl_root}/lib:/app/lib:/app/lib64:/usr/lib/x86_64-linux-gnu:/usr/lib:/usr/lib64"
                ),
            ),
            (
                "LIBGL_DRIVERS_PATH".to_string(),
                format!("{gl_root}/lib/dri"),
            ),
            (
                "GBM_BACKENDS_PATH".to_string(),
                format!("{gl_root}/lib/gbm"),
            ),
            (
                "__EGL_VENDOR_LIBRARY_DIRS".to_string(),
                format!("{gl_root}/share/glvnd/egl_vendor.d:/usr/share/glvnd/egl_vendor.d"),
            ),
            (
                "__EGL_EXTERNAL_PLATFORM_CONFIG_DIRS".to_string(),
                format!(
                    "/etc/egl/egl_external_platform.d:{gl_root}/egl/egl_external_platform.d:/usr/share/egl/egl_external_platform.d"
                ),
            ),
        ];

        if let Some(icd) = self.drm.as_ref().and_then(|drm| drm.device.vulkan_icd()) {
            let icd_path = format!("{gl_root}/lib/vulkan/icd.d/{icd}");
            env.push(("VK_DRIVER_FILES".to_string(), icd_path.clone()));
            env.push(("VK_ICD_FILENAMES".to_string(), icd_path));
        }
        if let Some(shim) = &self.wayland_drm_devt_shim {
            env.push((
                "FREEBSD_FLATPAK_DRM_DEV_T_MAP".to_string(),
                shim.dev_t_map.clone(),
            ));
        }

        env
    }

    pub fn ld_preload_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        if self.drm_syncobj_errno_shim.is_some() {
            paths.push(format!(
                "{GRAPHICS_SHIM_SANDBOX_DIR}/{DRM_SYNCOBJ_ERRNO_SHIM_LIB}"
            ));
        }
        if self.wayland_drm_devt_shim.is_some() {
            paths.push(format!(
                "{GRAPHICS_SHIM_SANDBOX_DIR}/{WAYLAND_DRM_DEVT_SHIM_LIB}"
            ));
        }
        paths
    }

    pub fn zypak_ld_preload_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        if self.wayland_drm_devt_shim.is_some() {
            paths.push(format!(
                "{GRAPHICS_SHIM_SANDBOX_DIR}/{WAYLAND_DRM_DEVT_SHIM_LIB}"
            ));
        }
        if self.drm_syncobj_errno_shim.is_some() {
            paths.push(format!(
                "{GRAPHICS_SHIM_SANDBOX_DIR}/{DRM_SYNCOBJ_ERRNO_SHIM_LIB}"
            ));
        }
        if self.chromium_zygote_drm_preload.is_some() {
            paths.push(format!(
                "{GRAPHICS_SHIM_SANDBOX_DIR}/{CHROMIUM_ZYGOTE_DRM_PRELOAD_LIB}"
            ));
        }
        paths
    }

    pub fn describe(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(gl) = &self.gl {
            lines.push(format!(
                "GL extension: {} -> /usr/{}",
                gl.checkout_dir.display(),
                gl.runtime_mount_relative.display()
            ));
            lines.push(format!("GL ref: {}", gl.ref_name));
        }
        if let Some(drm) = &self.drm {
            lines.push(format!(
                "DRM render node: /dev/dri/{} ({:}:{:})",
                drm.device.render_name, DRM_MAJOR, drm.device.render_minor
            ));
            lines.push(format!(
                "DRM render dev_t: host 0x{:x} -> linux 0x{:x}",
                drm.device.render_host_dev_t,
                linux_drm_dev_t(drm.device.render_minor)
            ));
            lines.push(format!(
                "DRM PCI: {} vendor={} device={} driver={}",
                drm.device.pci_slot, drm.device.vendor, drm.device.device, drm.device.driver
            ));
            if let Some(icd) = drm.device.vulkan_icd() {
                lines.push(format!("Vulkan ICD: {icd}"));
            }
            for mount in &drm.mounts {
                lines.push(format!(
                    "sysfs: {} -> {}",
                    mount.host_path.display(),
                    mount.sandbox_path.display()
                ));
            }
        }
        if let Some(shim) = &self.wayland_drm_devt_shim {
            lines.push(format!(
                "Wayland DRM dev_t shim: {} -> {}",
                shim.host_dir.display(),
                GRAPHICS_SHIM_SANDBOX_DIR
            ));
            lines.push(format!("Wayland DRM dev_t map: {}", shim.dev_t_map));
        }
        if let Some(shim) = &self.drm_syncobj_errno_shim {
            lines.push(format!(
                "DRM syncobj errno shim: {} -> {}/{}",
                shim.host_dir.display(),
                GRAPHICS_SHIM_SANDBOX_DIR,
                DRM_SYNCOBJ_ERRNO_SHIM_LIB
            ));
        }
        if let Some(shim) = &self.chromium_zygote_drm_preload {
            lines.push(format!(
                "Chromium zygote DRM preload: {} -> {}/{}",
                shim.host_dir.display(),
                GRAPHICS_SHIM_SANDBOX_DIR,
                CHROMIUM_ZYGOTE_DRM_PRELOAD_LIB
            ));
        }
        lines
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn cleanup(&self) -> Result<()> {
        if let Some(drm) = &self.drm {
            if drm.source_root.exists() {
                fs::remove_dir_all(&drm.source_root)
                    .with_context(|| format!("remove {}", drm.source_root.display()))?;
            }
        }
        Ok(())
    }
}

impl GraphicsMount {
    pub fn host_path(&self) -> &Path {
        &self.host_path
    }

    pub fn sandbox_target_relative(&self) -> Result<PathBuf> {
        self.sandbox_path
            .strip_prefix("/")
            .map(Path::to_path_buf)
            .with_context(|| {
                format!(
                    "graphics sandbox path is not absolute: {}",
                    self.sandbox_path.display()
                )
            })
    }
}

pub fn recover_stale_graphics_dirs(paths: &Installation) -> Result<()> {
    let root = paths.gpu();
    if !root.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(&root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            if entry
                .file_name()
                .to_string_lossy()
                .rsplit_once('-')
                .and_then(|(_, pid)| pid.parse::<i32>().ok())
                .is_some_and(process_alive)
            {
                continue;
            }
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    Ok(())
}

fn process_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(test)]
#[path = "tests/graphics.rs"]
mod tests;
