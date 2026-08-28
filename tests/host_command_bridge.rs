use std::path::PathBuf;
use std::process::Command;

#[test]
fn host_command_bridge_contract() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = std::env::temp_dir().join(format!(
        "freebsd-flatpak-host-command-test-{}",
        std::process::id()
    ));
    let pkg_config = Command::new("pkg-config")
        .args(["--cflags", "--libs", "gio-2.0", "gio-unix-2.0", "glib-2.0"])
        .output()
        .expect("run pkg-config for host command bridge test");
    assert!(pkg_config.status.success(), "pkg-config failed");
    let flags = String::from_utf8(pkg_config.stdout).expect("pkg-config output is UTF-8");

    let mut compiler = Command::new("cc");
    compiler
        .current_dir(&root)
        .args([
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "tests/host-command-bridge-test.c",
            "compatibility_helpers/portal_bridge/host_command.c",
        ])
        .arg("-o")
        .arg(&output)
        .args(flags.split_whitespace());
    assert!(
        compiler
            .status()
            .expect("compile host command bridge test")
            .success(),
        "host command bridge C test did not compile"
    );

    let status = Command::new(&output)
        .status()
        .expect("run host command bridge C test");
    let _ = std::fs::remove_file(&output);
    assert!(status.success(), "host command bridge test failed");
}
