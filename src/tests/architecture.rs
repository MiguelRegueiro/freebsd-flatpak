use super::*;

#[test]
fn normalizes_supported_freebsd_and_linux_machine_names() {
    for machine in ["amd64", "x86_64"] {
        assert_eq!(
            FlatpakArchitecture::from_host_machine(machine).unwrap(),
            FlatpakArchitecture::X86_64
        );
    }
    for machine in ["arm64", "aarch64"] {
        assert_eq!(
            FlatpakArchitecture::from_host_machine(machine).unwrap(),
            FlatpakArchitecture::Aarch64
        );
    }
}

#[test]
fn generates_linux_runtime_paths_for_each_supported_flatpak_architecture() {
    let amd64 = FlatpakArchitecture::X86_64;
    assert_eq!(amd64.runtime_libdir(), "lib/x86_64-linux-gnu");
    assert_eq!(
        amd64.vulkan_icd_filename("radeon"),
        "radeon_icd.x86_64.json"
    );

    let arm64 = FlatpakArchitecture::Aarch64;
    assert_eq!(arm64.runtime_libdir(), "lib/aarch64-linux-gnu");
    assert_eq!(
        arm64.vulkan_icd_filename("virtio"),
        "virtio_icd.aarch64.json"
    );
}

#[test]
fn obtains_architecture_from_runtime_ref() {
    assert_eq!(
        FlatpakArchitecture::from_runtime_ref("org.example.Platform/aarch64/25.08").unwrap(),
        FlatpakArchitecture::Aarch64
    );
}
