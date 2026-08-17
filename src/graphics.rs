use crate::runtime::{self, FlatpakApp, RuntimeGlExtension};
use anyhow::{bail, Context, Result};
use std::fs;
use std::os::unix::fs as unix_fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const DRM_MAJOR: u32 = 226;

#[derive(Debug, Clone)]
pub struct HostGraphics {
    gl: Option<RuntimeGlExtension>,
    drm: Option<DrmSysfsBridge>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GraphicsMount {
    host_path: PathBuf,
    sandbox_path: PathBuf,
}

#[derive(Debug, Clone)]
struct DrmSysfsBridge {
    source_root: PathBuf,
    mounts: Vec<GraphicsMount>,
    device: DrmDevice,
}

#[derive(Debug, Clone)]
struct DrmDevice {
    card_name: String,
    card_minor: u32,
    render_name: String,
    render_minor: u32,
    pci_slot: String,
    vendor: String,
    device: String,
    class: String,
    revision: String,
    subsystem_vendor: String,
    subsystem_device: String,
    driver: String,
}

#[derive(Debug, Clone)]
struct PciInfo {
    class: String,
    revision: String,
    vendor: String,
    device: String,
    subsystem_vendor: String,
    subsystem_device: String,
}

impl HostGraphics {
    pub fn prepare(project_root: &Path, app: &FlatpakApp) -> Result<Self> {
        let mut warnings = Vec::new();
        let gl =
            runtime::ensure_default_gl_extension(project_root, &app.runtime_ref, &app.runtime_dir)?;
        let drm = if gl.is_some() {
            match DrmDevice::detect() {
                Ok(device) => Some(DrmSysfsBridge::prepare(project_root, &app.app_id, device)?),
                Err(error) => {
                    warnings.push(format!("DRM sysfs bridge disabled: {error:#}"));
                    None
                }
            }
        } else {
            None
        };

        Ok(Self { gl, drm, warnings })
    }

    pub fn runtime_mounts(&self) -> Vec<GraphicsMount> {
        self.gl
            .iter()
            .map(|gl| GraphicsMount {
                host_path: gl.checkout_dir.join("files"),
                sandbox_path: PathBuf::from("/usr").join(&gl.runtime_mount_relative),
            })
            .collect()
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
        vec![
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
        ]
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
                "DRM PCI: {} vendor={} device={} driver={}",
                drm.device.pci_slot, drm.device.vendor, drm.device.device, drm.device.driver
            ));
            for mount in &drm.mounts {
                lines.push(format!(
                    "sysfs: {} -> {}",
                    mount.host_path.display(),
                    mount.sandbox_path.display()
                ));
            }
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

impl DrmSysfsBridge {
    fn prepare(project_root: &Path, app_id: &str, device: DrmDevice) -> Result<Self> {
        let source_root = project_root.join("runtime").join("gpu").join(format!(
            "{}-{}",
            safe_name(app_id),
            std::process::id()
        ));
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

impl DrmDevice {
    fn detect() -> Result<Self> {
        let render = first_dri_node("renderD").context("no /dev/dri/renderD* node found")?;
        let card = matching_card_node(&render).context("no matching /dev/dri/card* node found")?;
        let pci_slot = sysctl_value(&format!("hw.dri.{}.busid", card.index))
            .ok()
            .and_then(|value| value.strip_prefix("pci:").map(ToOwned::to_owned))
            .context("could not determine DRM PCI bus id")?;
        let pci_id = sysctl_value(&format!("dev.drm.{}.PCI_ID", render.minor))
            .or_else(|_| sysctl_value(&format!("dev.drm.{}.PCI_ID", card.minor)))
            .context("could not determine DRM PCI vendor/device id")?;
        let (vendor, device_id) = pci_id
            .split_once(':')
            .map(|(vendor, device)| (hex4(vendor), hex4(device)))
            .context("unexpected dev.drm.*.PCI_ID format")?;
        let pci = pciconf_info(&pci_slot).unwrap_or_else(|_| PciInfo {
            class: "0x030000".to_string(),
            revision: "0x00".to_string(),
            vendor: vendor.clone(),
            device: device_id.clone(),
            subsystem_vendor: "0x0000".to_string(),
            subsystem_device: "0x0000".to_string(),
        });
        let driver = sysctl_value(&format!("hw.dri.{}.name", card.index))
            .ok()
            .and_then(|name| name.split_whitespace().next().map(ToOwned::to_owned))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "drm".to_string());

        Ok(Self {
            card_name: card.name,
            card_minor: card.minor,
            render_name: render.name,
            render_minor: render.minor,
            pci_slot,
            vendor: pci.vendor,
            device: pci.device,
            class: pci.class,
            revision: pci.revision,
            subsystem_vendor: pci.subsystem_vendor,
            subsystem_device: pci.subsystem_device,
            driver,
        })
    }
}

#[derive(Debug, Clone)]
struct DriNode {
    name: String,
    index: u32,
    minor: u32,
}

fn first_dri_node(prefix: &str) -> Result<DriNode> {
    let mut nodes = Vec::new();
    for entry in fs::read_dir("/dev/dri").context("read /dev/dri")? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(number) = name
            .strip_prefix(prefix)
            .and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        nodes.push(DriNode {
            minor: number,
            index: if prefix == "renderD" {
                number.saturating_sub(128)
            } else {
                number
            },
            name,
        });
    }
    nodes.sort_by_key(|node| node.minor);
    nodes
        .into_iter()
        .next()
        .with_context(|| format!("no {prefix} node"))
}

fn matching_card_node(render: &DriNode) -> Result<DriNode> {
    let render_pci = sysctl_value(&format!("dev.drm.{}.PCI_ID", render.minor)).ok();
    let mut fallback = None;
    for entry in fs::read_dir("/dev/dri").context("read /dev/dri")? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(number) = name
            .strip_prefix("card")
            .and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        let node = DriNode {
            name,
            index: number,
            minor: number,
        };
        if fallback.is_none() {
            fallback = Some(node.clone());
        }
        let card_pci = sysctl_value(&format!("dev.drm.{}.PCI_ID", node.minor)).ok();
        if render_pci.is_some() && card_pci == render_pci {
            return Ok(node);
        }
    }
    fallback.context("no card node")
}

fn write_linux_drm_sysfs(
    bus: &Path,
    dev_char: &Path,
    class_drm: &Path,
    device: &DrmDevice,
) -> Result<()> {
    let pci_device = bus.join("pci").join("devices").join(&device.pci_slot);
    fs::create_dir_all(pci_device.join("drm").join(&device.card_name))?;
    fs::create_dir_all(pci_device.join("drm").join(&device.render_name))?;
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

fn pciconf_info(pci_slot: &str) -> Result<PciInfo> {
    let locator = pciconf_locator(pci_slot)?;
    let output = Command::new("pciconf")
        .arg("-lv")
        .arg(locator)
        .output()
        .context("run pciconf -lv")?;
    if !output.status.success() {
        bail!("pciconf -lv failed with status {}", output.status);
    }
    let text = String::from_utf8(output.stdout)?;
    let first = text.lines().next().context("empty pciconf output")?;
    let get = |key: &str| -> Result<String> {
        first
            .split_whitespace()
            .find_map(|part| part.strip_prefix(&format!("{key}=")))
            .map(ToOwned::to_owned)
            .with_context(|| format!("pciconf output missing {key}"))
    };
    Ok(PciInfo {
        class: hex_prefixed(&get("class")?, 6),
        revision: hex_prefixed(&get("rev")?, 2),
        vendor: hex_prefixed(&get("vendor")?, 4),
        device: hex_prefixed(&get("device")?, 4),
        subsystem_vendor: hex_prefixed(&get("subvendor")?, 4),
        subsystem_device: hex_prefixed(&get("subdevice")?, 4),
    })
}

fn pciconf_locator(pci_slot: &str) -> Result<String> {
    let mut slot_parts = pci_slot.split(':');
    let _domain = slot_parts.next().context("missing PCI domain")?;
    let bus = slot_parts.next().context("missing PCI bus")?;
    let device_function = slot_parts.next().context("missing PCI device/function")?;
    let (device, function) = device_function
        .split_once('.')
        .context("missing PCI function")?;
    Ok(format!(
        "pci0:{}:{}:{}",
        parse_hex_or_decimal(bus)?,
        parse_hex_or_decimal(device)?,
        parse_hex_or_decimal(function)?
    ))
}

fn sysctl_value(name: &str) -> Result<String> {
    let output = Command::new("sysctl")
        .arg("-n")
        .arg(name)
        .output()
        .with_context(|| format!("run sysctl -n {name}"))?;
    if !output.status.success() {
        bail!("sysctl -n {name} failed with status {}", output.status);
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
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

fn parse_hex_or_decimal(value: &str) -> Result<u32> {
    u32::from_str_radix(value, 16)
        .or_else(|_| value.parse::<u32>())
        .with_context(|| format!("parse PCI number {value}"))
}

fn hex4(value: &str) -> String {
    hex_prefixed(value, 4)
}

fn hex_prefixed(value: &str, width: usize) -> String {
    let value = value.trim().trim_start_matches("0x");
    format!("0x{:0>width$}", value.to_ascii_lowercase())
}

fn trim_hex(value: &str) -> &str {
    value.trim().trim_start_matches("0x")
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

pub fn recover_stale_graphics_dirs(project_root: &Path) -> Result<()> {
    let root = project_root.join("runtime").join("gpu");
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
mod tests {
    use super::{hex_prefixed, parse_hex_or_decimal, pciconf_locator};

    #[test]
    fn converts_linux_pci_slot_to_freebsd_locator() {
        assert_eq!(pciconf_locator("0000:00:02.0").unwrap(), "pci0:0:2:0");
    }

    #[test]
    fn normalizes_hex_values() {
        assert_eq!(hex_prefixed("8086", 4), "0x8086");
        assert_eq!(hex_prefixed("0x2", 2), "0x02");
    }

    #[test]
    fn parses_pci_numbers_as_hex() {
        assert_eq!(parse_hex_or_decimal("1c").unwrap(), 28);
        assert_eq!(parse_hex_or_decimal("02").unwrap(), 2);
    }
}
