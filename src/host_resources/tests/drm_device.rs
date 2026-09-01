use super::*;
use crate::architecture::FlatpakArchitecture;

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

#[test]
fn chooses_intel_vulkan_icd_from_pci_vendor() {
    let device = DrmDevice {
        card_name: "card0".to_string(),
        card_minor: 0,
        card_host_dev_t: 0x61,
        render_name: "renderD128".to_string(),
        render_minor: 128,
        render_host_dev_t: 0x100,
        pci_slot: "0000:00:02.0".to_string(),
        vendor: "0x8086".to_string(),
        device: "0x9b41".to_string(),
        class: "0x030000".to_string(),
        revision: "0x00".to_string(),
        subsystem_vendor: "0x0000".to_string(),
        subsystem_device: "0x0000".to_string(),
        driver: "i915".to_string(),
    };

    assert_eq!(
        device.vulkan_icd(FlatpakArchitecture::X86_64).as_deref(),
        Some("intel_icd.x86_64.json")
    );
    assert_eq!(
        device.vulkan_icd(FlatpakArchitecture::Aarch64).as_deref(),
        Some("intel_icd.aarch64.json")
    );
}

#[test]
fn maps_host_drm_dev_t_values_to_linux_drm_dev_t_values() {
    let device = DrmDevice {
        card_name: "card0".to_string(),
        card_minor: 0,
        card_host_dev_t: 0x61,
        render_name: "renderD128".to_string(),
        render_minor: 128,
        render_host_dev_t: 0x100,
        pci_slot: "0000:00:02.0".to_string(),
        vendor: "0x8086".to_string(),
        device: "0x9b41".to_string(),
        class: "0x030000".to_string(),
        revision: "0x00".to_string(),
        subsystem_vendor: "0x0000".to_string(),
        subsystem_device: "0x0000".to_string(),
        driver: "i915".to_string(),
    };

    assert_eq!(device.dev_t_map(), "0x61=0xe200,0x100=0xe280");
}
