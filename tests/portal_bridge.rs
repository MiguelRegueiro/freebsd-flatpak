use std::path::PathBuf;
use std::process::Command;

#[test]
fn portal_bridge_screencast_contract() {
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
        ])
        .output()
        .expect("run pkg-config for portal bridge test");
    assert!(pkg_config.status.success(), "pkg-config failed");
    let flags = String::from_utf8(pkg_config.stdout).expect("pkg-config output is UTF-8");

    let mut compiler = Command::new("cc");
    compiler
        .current_dir(&root)
        .arg("tests/portal-bridge-test.c")
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
