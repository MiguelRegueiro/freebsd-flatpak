use crate::installation::installation_paths::Installation;
use crate::installation::{self as runtime, FlatpakApp, RuntimeVaapiExtension};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct HostVideo {
    vaapi: Option<RuntimeVaapiExtension>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct VideoMount {
    host_path: PathBuf,
    sandbox_path: PathBuf,
}

impl HostVideo {
    pub fn prepare(paths: &Installation, app: &FlatpakApp) -> Result<Self> {
        let mut warnings = Vec::new();
        let vaapi = if host_has_intel_drm_device() {
            runtime::activate_intel_vaapi_extension(paths, &app.runtime_ref, &app.runtime_dir)?
        } else {
            warnings.push(
                "Intel VAAPI extension disabled: no Intel DRM render node detected".to_string(),
            );
            None
        };

        Ok(Self { vaapi, warnings })
    }

    pub fn extension_refs(&self) -> impl Iterator<Item = &str> {
        self.vaapi.iter().map(|vaapi| vaapi.ref_name())
    }

    pub fn runtime_mounts(&self) -> Vec<VideoMount> {
        self.vaapi
            .iter()
            .map(|vaapi| VideoMount {
                host_path: vaapi.checkout_dir.join("files"),
                sandbox_path: PathBuf::from("/usr").join(&vaapi.runtime_mount_relative),
            })
            .collect()
    }

    pub fn ld_library_paths(&self) -> Vec<String> {
        self.vaapi
            .iter()
            .filter_map(|vaapi| {
                vaapi.ld_library_relative.as_ref().map(|relative| {
                    PathBuf::from("/usr")
                        .join(&vaapi.runtime_mount_relative)
                        .join(relative)
                        .display()
                        .to_string()
                })
            })
            .collect()
    }

    pub fn env(&self) -> Vec<(String, String)> {
        if self.vaapi.is_none() {
            return Vec::new();
        }

        let mut env = vec![(
            "LIBVA_DRIVERS_PATH".to_string(),
            "/usr/lib/x86_64-linux-gnu/dri/intel-vaapi-driver:/usr/lib/x86_64-linux-gnu/dri:/usr/lib/dri"
                .to_string(),
        )];
        if let Ok(driver) = std::env::var("LIBVA_DRIVER_NAME") {
            if !driver.is_empty() {
                env.push(("LIBVA_DRIVER_NAME".to_string(), driver));
            }
        }
        env
    }

    pub fn describe(&self) -> Vec<String> {
        self.vaapi
            .iter()
            .flat_map(|vaapi| {
                let mut lines = vec![
                    format!(
                        "VAAPI Intel extension: {} -> /usr/{}",
                        vaapi.checkout_dir.display(),
                        vaapi.runtime_mount_relative.display()
                    ),
                    format!("VAAPI Intel ref: {}", vaapi.ref_name),
                ];
                if let Some(path) = &vaapi.ld_library_relative {
                    lines.push(format!(
                        "VAAPI library path: /usr/{}/{}",
                        vaapi.runtime_mount_relative.display(),
                        path.display()
                    ));
                }
                lines
            })
            .collect()
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

impl VideoMount {
    pub fn host_path(&self) -> &Path {
        &self.host_path
    }

    pub fn sandbox_target_relative(&self) -> Result<PathBuf> {
        self.sandbox_path
            .strip_prefix("/")
            .map(Path::to_path_buf)
            .with_context(|| {
                format!(
                    "video sandbox path is not absolute: {}",
                    self.sandbox_path.display()
                )
            })
    }
}

pub(crate) fn host_has_intel_drm_device() -> bool {
    drm_render_minors()
        .into_iter()
        .any(|minor| drm_pci_id(minor).is_some_and(|pci_id| pci_id.starts_with("8086:")))
}

fn drm_render_minors() -> Vec<u32> {
    let Ok(entries) = fs::read_dir("/dev/dri") else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_prefix("renderD"))
                .and_then(|minor| minor.parse::<u32>().ok())
        })
        .collect()
}

fn drm_pci_id(minor: u32) -> Option<String> {
    let output = Command::new("sysctl")
        .arg("-n")
        .arg(format!("dev.drm.{minor}.PCI_ID"))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
#[path = "tests/video_acceleration.rs"]
mod tests;
