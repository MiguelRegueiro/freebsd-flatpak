use super::drm_device::{trim_hex, DrmDevice};
use super::graphics::{GraphicsMount, DRM_MAJOR};
use crate::installation::installation_paths::Installation;
use anyhow::{Context, Result};
use std::fs;
use std::os::unix::fs as unix_fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub(super) struct DrmSysfsBridge {
    pub(super) source_root: PathBuf,
    pub(super) mounts: Vec<GraphicsMount>,
    pub(super) device: DrmDevice,
}
impl DrmSysfsBridge {
    pub(super) fn prepare(
        paths: &Installation,
        app_id: &str,
        instance_id: &str,
        device: DrmDevice,
    ) -> Result<Self> {
        let source_root =
            paths
                .gpu()
                .join(format!("{}-{}", safe_name(app_id), safe_name(instance_id)));
        if source_root.exists() {
            fs::remove_dir_all(&source_root)
                .with_context(|| format!("replace {}", source_root.display()))?;
        }

        let bus = source_root.join("sys-bus");
        let dev_char = source_root.join("sys-dev-char");
        let class_drm = source_root.join("sys-class-drm");
        write_linux_drm_sysfs(&bus, &dev_char, &class_drm, &device)?;

        Ok(Self {
            mounts: vec![
                GraphicsMount {
                    host_path: bus,
                    sandbox_path: PathBuf::from("/sys/bus"),
                },
                GraphicsMount {
                    host_path: dev_char,
                    sandbox_path: PathBuf::from("/sys/dev/char"),
                },
                GraphicsMount {
                    host_path: class_drm,
                    sandbox_path: PathBuf::from("/sys/class/drm"),
                },
            ],
            source_root,
            device,
        })
    }
}

pub(super) fn write_linux_drm_sysfs(
    bus: &Path,
    dev_char: &Path,
    class_drm: &Path,
    device: &DrmDevice,
) -> Result<()> {
    let pci_device = bus.join("pci").join("devices").join(&device.pci_slot);
    let pci_driver = bus.join("pci").join("drivers").join(&device.driver);
    fs::create_dir_all(pci_device.join("drm").join(&device.card_name))?;
    fs::create_dir_all(pci_device.join("drm").join(&device.render_name))?;
    fs::create_dir_all(&pci_driver)?;
    write_file(pci_device.join("vendor"), &format!("{}\n", device.vendor))?;
    write_file(pci_device.join("device"), &format!("{}\n", device.device))?;
    write_file(
        pci_device.join("subsystem_vendor"),
        &format!("{}\n", device.subsystem_vendor),
    )?;
    write_file(
        pci_device.join("subsystem_device"),
        &format!("{}\n", device.subsystem_device),
    )?;
    write_file(
        pci_device.join("revision"),
        &format!("{}\n", device.revision),
    )?;
    write_file(pci_device.join("class"), &format!("{}\n", device.class))?;
    symlink_replace("/sys/bus/pci", &pci_device.join("subsystem"))?;
    symlink_replace(
        format!("/sys/bus/pci/drivers/{}", device.driver),
        &pci_device.join("driver"),
    )?;
    symlink_replace(
        format!("/sys/bus/pci/devices/{}", device.pci_slot),
        &pci_driver.join(&device.pci_slot),
    )?;
    write_file(
        pci_device.join("uevent"),
        &format!(
            "DRIVER={}\nPCI_CLASS={}\nPCI_ID={}:{}\nPCI_SUBSYS_ID={}:{}\nPCI_SLOT_NAME={}\nMODALIAS=pci:v0000{}d0000{}sv0000{}sd0000{}bc{}sc00i00\n",
            device.driver,
            trim_hex(&device.class),
            trim_hex(&device.vendor).to_ascii_uppercase(),
            trim_hex(&device.device).to_ascii_uppercase(),
            trim_hex(&device.subsystem_vendor).to_ascii_uppercase(),
            trim_hex(&device.subsystem_device).to_ascii_uppercase(),
            device.pci_slot,
            trim_hex(&device.vendor).to_ascii_uppercase(),
            trim_hex(&device.device).to_ascii_uppercase(),
            trim_hex(&device.subsystem_vendor).to_ascii_uppercase(),
            trim_hex(&device.subsystem_device).to_ascii_uppercase(),
            trim_hex(&device.class).get(0..2).unwrap_or("03"),
        ),
    )?;

    write_dev_char_node(
        dev_char,
        &device.card_name,
        device.card_minor,
        &device.pci_slot,
    )?;
    write_dev_char_node(
        dev_char,
        &device.render_name,
        device.render_minor,
        &device.pci_slot,
    )?;
    write_class_drm_node(
        class_drm,
        &device.card_name,
        device.card_minor,
        &device.pci_slot,
    )?;
    write_class_drm_node(
        class_drm,
        &device.render_name,
        device.render_minor,
        &device.pci_slot,
    )?;
    Ok(())
}

fn write_dev_char_node(dev_char: &Path, name: &str, minor: u32, pci_slot: &str) -> Result<()> {
    let dir = dev_char.join(format!("{DRM_MAJOR}:{minor}"));
    fs::create_dir_all(&dir)?;
    symlink_replace(
        format!("/sys/bus/pci/devices/{pci_slot}"),
        &dir.join("device"),
    )?;
    write_file(
        dir.join("uevent"),
        &format!("MAJOR={DRM_MAJOR}\nMINOR={minor}\nDEVNAME=dri/{name}\n"),
    )
}

pub(super) fn linux_drm_dev_t(minor: u32) -> u64 {
    linux_makedev(DRM_MAJOR as u64, minor as u64)
}

fn linux_makedev(major: u64, minor: u64) -> u64 {
    ((major & 0xfff) << 8) | (minor & 0xff) | ((minor & !0xff) << 12)
}

fn write_class_drm_node(class_drm: &Path, name: &str, minor: u32, pci_slot: &str) -> Result<()> {
    let dir = class_drm.join(name);
    fs::create_dir_all(&dir)?;
    symlink_replace(
        format!("/sys/bus/pci/devices/{pci_slot}"),
        &dir.join("device"),
    )?;
    write_file(dir.join("dev"), &format!("{DRM_MAJOR}:{minor}\n"))?;
    write_file(
        dir.join("uevent"),
        &format!("MAJOR={DRM_MAJOR}\nMINOR={minor}\nDEVNAME=dri/{name}\n"),
    )
}

fn write_file(path: PathBuf, data: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, data).with_context(|| format!("write {}", path.display()))
}

fn symlink_replace(target: impl AsRef<Path>, link: &Path) -> Result<()> {
    if fs::symlink_metadata(link).is_ok() {
        fs::remove_file(link).with_context(|| format!("replace {}", link.display()))?;
    }
    unix_fs::symlink(target, link).with_context(|| format!("symlink {}", link.display()))
}

fn safe_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/linux_drm_sysfs.rs"]
mod tests;
