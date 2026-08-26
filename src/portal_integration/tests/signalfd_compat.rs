use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn signalfd_compat_supports_the_flatpak_spawn_contract() {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output_dir = project_root.join("target/portal-tests");
    fs::create_dir_all(&output_dir).unwrap();
    let output = output_dir.join("signalfd-compat-test");
    let compile = Command::new("/compat/linux/usr/bin/gcc")
        .args(["-O2", "-Wall", "-Wextra", "-Werror"])
        .arg(project_root.join("compatibility_helpers/signalfd-compat.c"))
        .arg(project_root.join("tests/signalfd-compat-test.c"))
        .args(["-pthread", "-o"])
        .arg(&output)
        .env(
            "PATH",
            "/compat/linux/usr/bin:/compat/linux/bin:/usr/bin:/bin",
        )
        .status()
        .expect("compile signalfd compatibility test");
    assert!(compile.success());

    let run = Command::new(&output)
        .status()
        .expect("run signalfd compatibility test");
    assert!(run.success());
}
