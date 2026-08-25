use super::*;
use std::fs;
use std::process::Command;

#[test]
fn network_permission_controls_mount_and_preload() {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-network-test-{}",
        std::process::id()
    ));
    let libexec = root.join("libexec");
    fs::create_dir_all(&libexec).unwrap();
    fs::write(libexec.join(NETLINK_ROUTE_FLAGS_SHIM_LIB), []).unwrap();
    let paths = Installation::for_test(&root);

    let disabled = HostNetwork::prepare(&paths, false).unwrap();
    assert!(disabled.runtime_mount().is_none());
    assert!(disabled.preload_paths().is_empty());

    let enabled = HostNetwork::prepare(&paths, true).unwrap();
    let mount = enabled.runtime_mount().unwrap();
    assert_eq!(mount.host_path(), libexec);
    assert_eq!(
        mount.sandbox_target_relative().unwrap(),
        PathBuf::from("run/host/freebsd-flatpak")
    );
    assert_eq!(
        enabled.preload_paths(),
        vec!["/run/host/freebsd-flatpak/libnetlink-route-flags-shim.so"]
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn netlink_route_flags_shim_only_repairs_active_route_links() {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output_dir = project_root.join("target/network-tests");
    fs::create_dir_all(&output_dir).unwrap();
    let output = output_dir.join("netlink-route-flags-shim-test");
    let compile = Command::new("/compat/linux/usr/bin/gcc")
        .args(["-O2", "-Wall", "-Wextra", "-Werror"])
        .arg("-DNETLINK_ROUTE_FLAGS_SHIM_TEST")
        .arg(project_root.join("compatibility_helpers/netlink-route-flags-shim.c"))
        .arg(project_root.join("tests/netlink-route-flags-shim-test.c"))
        .arg("-o")
        .arg(&output)
        .env(
            "PATH",
            "/compat/linux/usr/bin:/compat/linux/bin:/usr/bin:/bin",
        )
        .status()
        .expect("compile netlink route flags shim test");
    assert!(compile.success());

    let run = Command::new(&output)
        .status()
        .expect("run netlink route flags shim test");
    assert!(run.success());
}
