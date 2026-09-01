use crate::installation::ExtensionMount;
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct HostVideo {
    vaapi: Option<ExtensionMount>,
    warnings: Vec<String>,
}

impl HostVideo {
    pub fn prepare(vaapi: Option<ExtensionMount>) -> Result<Self> {
        let mut warnings = Vec::new();
        if !host_has_intel_drm_device() {
            warnings.push(
                "Intel VAAPI extension disabled: no Intel DRM render node detected".to_string(),
            );
        }

        Ok(Self { vaapi, warnings })
    }

    pub fn ld_library_paths(&self) -> Vec<String> {
        Vec::new()
    }

    pub fn env(&self) -> Vec<(String, String)> {
        let Some(vaapi) = &self.vaapi else {
            return Vec::new();
        };

        let extension_dir = PathBuf::from("/").join(&vaapi.target);
        let dri_dir = extension_dir
            .parent()
            .unwrap_or(Path::new("/usr/lib/dri"))
            .to_path_buf();
        let mut driver_paths = vec![extension_dir, dri_dir, PathBuf::from("/usr/lib/dri")];
        driver_paths.dedup();

        let mut env = vec![(
            "LIBVA_DRIVERS_PATH".to_string(),
            driver_paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(":"),
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
                        "VAAPI extension: {} -> /{}",
                        vaapi.checkout_dir.display(),
                        vaapi.target.display()
                    ),
                    format!("VAAPI Intel ref: {}", vaapi.ref_name),
                ];
                for path in &vaapi.add_ld_paths {
                    lines.push(format!("VAAPI library path: {path}"));
                }
                lines
            })
            .collect()
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
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
