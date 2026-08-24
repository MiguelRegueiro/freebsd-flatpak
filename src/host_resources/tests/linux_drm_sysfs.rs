use super::*;
use crate::host_resources::drm_device::DrmDevice;
use std::fs;
use std::path::PathBuf;

#[test]
fn encodes_linux_drm_dev_t_values() {
    assert_eq!(linux_drm_dev_t(0), 0xe200);
    assert_eq!(linux_drm_dev_t(128), 0xe280);
}

#[test]
fn writes_pci_driver_links_for_drm_sysfs() {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-graphics-test-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let bus = root.join("sys-bus");
    let dev_char = root.join("sys-dev-char");
    let class_drm = root.join("sys-class-drm");
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
        revision: "0x02".to_string(),
        subsystem_vendor: "0x1028".to_string(),
        subsystem_device: "0x096e".to_string(),
        driver: "i915".to_string(),
    };

    write_linux_drm_sysfs(&bus, &dev_char, &class_drm, &device).unwrap();

    let pci_device = bus.join("pci/devices/0000:00:02.0");
    assert_eq!(
        fs::read_link(pci_device.join("driver")).unwrap(),
        PathBuf::from("/sys/bus/pci/drivers/i915")
    );
    assert_eq!(
        fs::read_link(bus.join("pci/drivers/i915/0000:00:02.0")).unwrap(),
        PathBuf::from("/sys/bus/pci/devices/0000:00:02.0")
    );
    assert!(fs::read_to_string(pci_device.join("uevent"))
        .unwrap()
        .contains("DRIVER=i915"));

    let _ = fs::remove_dir_all(&root);
}
