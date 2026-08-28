use std::path::PathBuf;
use std::process::Command;

#[test]
fn open_uri_portal_contract() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = std::env::temp_dir().join(format!(
        "freebsd-flatpak-open-uri-portal-test-{}",
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
        ])
        .output()
        .expect("run pkg-config for OpenURI portal test");
    assert!(pkg_config.status.success(), "pkg-config failed");
    let flags = String::from_utf8(pkg_config.stdout).expect("pkg-config output is UTF-8");

    let status = Command::new("cc")
        .current_dir(&root)
        .args([
            "-Wall",
            "-Wextra",
            "-Werror",
            "tests/open-uri-portal-test.c",
            "compatibility_helpers/portal_bridge/basic_desktop_portals.c",
            "compatibility_helpers/portal_bridge/open_uri_portal.c",
            "compatibility_helpers/portal_bridge/portal_request.c",
        ])
        .arg("-o")
        .arg(&output)
        .args(flags.split_whitespace())
        .status()
        .expect("compile OpenURI portal C test");
    assert!(status.success(), "OpenURI portal C test did not compile");

    let status = Command::new(&output)
        .status()
        .expect("run OpenURI portal C test");
    let _ = std::fs::remove_file(&output);
    assert!(status.success(), "OpenURI portal C test failed");
}
