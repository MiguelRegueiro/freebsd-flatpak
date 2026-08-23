use super::linux_drm_sysfs::linux_drm_dev_t;
use anyhow::{bail, Context, Result};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub(super) struct DrmDevice {
    pub(super) card_name: String,
    pub(super) card_minor: u32,
    pub(super) card_host_dev_t: u64,
    pub(super) render_name: String,
    pub(super) render_minor: u32,
    pub(super) render_host_dev_t: u64,
    pub(super) pci_slot: String,
    pub(super) vendor: String,
    pub(super) device: String,
    pub(super) class: String,
    pub(super) revision: String,
    pub(super) subsystem_vendor: String,
    pub(super) subsystem_device: String,
    pub(super) driver: String,
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

impl DrmDevice {
    pub(super) fn detect() -> Result<Self> {
        let render = first_dri_node("renderD").context("no /dev/dri/renderD* node found")?;
        let card = matching_card_node(&render).context("no matching /dev/dri/card* node found")?;
        let card_host_dev_t = dri_node_dev_t(&card)?;
        let render_host_dev_t = dri_node_dev_t(&render)?;
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
            card_host_dev_t,
            render_name: render.name,
            render_minor: render.minor,
            render_host_dev_t,
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

    pub(super) fn vulkan_icd(&self) -> Option<&'static str> {
        match trim_hex(&self.vendor) {
            "8086" => Some("intel_icd.x86_64.json"),
            "1002" | "1022" => Some("radeon_icd.x86_64.json"),
            "1af4" => Some("virtio_icd.x86_64.json"),
            _ => None,
        }
    }

    pub(super) fn dev_t_map(&self) -> String {
        [
            (self.card_host_dev_t, linux_drm_dev_t(self.card_minor)),
            (self.render_host_dev_t, linux_drm_dev_t(self.render_minor)),
        ]
        .into_iter()
        .map(|(host, linux)| format!("0x{host:x}=0x{linux:x}"))
        .collect::<Vec<_>>()
        .join(",")
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

fn dri_node_dev_t(node: &DriNode) -> Result<u64> {
    let path = Path::new("/dev/dri").join(&node.name);
    Ok(fs::metadata(&path)
        .with_context(|| format!("stat {}", path.display()))?
        .rdev())
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

pub(super) fn pciconf_locator(pci_slot: &str) -> Result<String> {
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

pub(super) fn parse_hex_or_decimal(value: &str) -> Result<u32> {
    u32::from_str_radix(value, 16)
        .or_else(|_| value.parse::<u32>())
        .with_context(|| format!("parse PCI number {value}"))
}

fn hex4(value: &str) -> String {
    hex_prefixed(value, 4)
}

pub(super) fn hex_prefixed(value: &str, width: usize) -> String {
    let value = value.trim().trim_start_matches("0x");
    format!("0x{:0>width$}", value.to_ascii_lowercase())
}

pub(super) fn trim_hex(value: &str) -> &str {
    value.trim().trim_start_matches("0x")
}

#[cfg(test)]
#[path = "tests/drm_device.rs"]
mod tests;
