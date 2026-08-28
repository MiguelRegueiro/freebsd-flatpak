use std::path::PathBuf;
use std::process::Command;

#[test]
fn compatibility_bridge_contract() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = std::env::temp_dir().join(format!(
        "freebsd-flatpak-portal-bridge-test-{}",
        std::process::id()
    ));
    let pkg_config = Command::new("pkg-config")
        .args([
            "--cflags",
            "--libs",
            "gio-2.0",
            "gio-unix-2.0",
            "glib-2.0",
            "libpipewire-0.3",
            "gdk-pixbuf-2.0",
        ])
        .output()
        .expect("run pkg-config for portal bridge test");
    assert!(pkg_config.status.success(), "pkg-config failed");
    let flags = String::from_utf8(pkg_config.stdout).expect("pkg-config output is UTF-8");

    let mut compiler = Command::new("cc");
    compiler
        .current_dir(&root)
        .args([
            "tests/portal-bridge-test.c",
            "compatibility_helpers/portal_bridge/basic_desktop_portals.c",
            "compatibility_helpers/portal_bridge/document_grant_store.c",
            "compatibility_helpers/portal_bridge/document_grant_persistence.c",
            "compatibility_helpers/portal_bridge/document_id.c",
            "compatibility_helpers/portal_bridge/document_mounts.c",
            "compatibility_helpers/portal_bridge/document_portal.c",
            "compatibility_helpers/portal_bridge/file_chooser_portal.c",
            "compatibility_helpers/portal_bridge/host_command.c",
            "compatibility_helpers/portal_bridge/pipewire_screencast_linker.c",
            "compatibility_helpers/portal_bridge/portal_bridge_process.c",
            "compatibility_helpers/portal_bridge/portal_request.c",
            "compatibility_helpers/portal_bridge/sandbox_document_registration.c",
            "compatibility_helpers/portal_bridge/screencast_portal.c",
            "compatibility_helpers/status_notifier_bridge/dbusmenu_proxy.c",
            "compatibility_helpers/status_notifier_bridge/icon_resolver.c",
            "compatibility_helpers/status_notifier_bridge/status_notifier_item_proxy.c",
            "compatibility_helpers/status_notifier_bridge/status_notifier_watcher.c",
        ])
        .arg("-o")
        .arg(&output)
        .args(flags.split_whitespace());
    let compile_status = compiler.status().expect("compile portal bridge C test");
    assert!(
        compile_status.success(),
        "portal bridge C test did not compile"
    );

    let test_status = Command::new(&output)
        .status()
        .expect("run portal bridge C test");
    let _ = std::fs::remove_file(&output);
    assert!(test_status.success(), "portal bridge C test failed");
}
