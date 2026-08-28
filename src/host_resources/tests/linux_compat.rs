use super::*;
use std::fs;
use std::process::Command;

#[test]
fn linux_compat_helpers_are_mounted_and_wrapper_is_on_path() {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-linux-compat-test-{}",
        std::process::id()
    ));
    let libexec = root.join("libexec");
    fs::create_dir_all(&libexec).unwrap();
    fs::create_dir_all(libexec.join("linux-bin")).unwrap();
    fs::write(libexec.join(SIGNALFD_SHIM_LIB), []).unwrap();
    fs::write(libexec.join(FLATPAK_SPAWN_WRAPPER), []).unwrap();
    let paths = Installation::for_test(&root);

    let compatibility = HostLinuxCompat::prepare(&paths).unwrap();
    let (source, target) = compatibility.runtime_mount();
    assert_eq!(source, libexec);
    assert_eq!(target, PathBuf::from("run/host/freebsd-flatpak"));
    assert_eq!(
        compatibility.path_entries(),
        vec!["/run/host/freebsd-flatpak/linux-bin"]
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn signalfd_shim_emulates_create_read_update_and_close() {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output_dir = project_root.join("target/signalfd-tests");
    fs::create_dir_all(&output_dir).unwrap();
    let output = output_dir.join("signalfd-shim-test");
    let compile = Command::new("/compat/linux/usr/bin/gcc")
        .args(["-O2", "-Wall", "-Wextra", "-Werror"])
        .arg("-DSIGNALFD_SHIM_TEST")
        .arg(project_root.join("compatibility_helpers/signalfd-shim.c"))
        .arg(project_root.join("tests/signalfd-shim-test.c"))
        .arg("-o")
        .arg(&output)
        .arg("-pthread")
        .env(
            "PATH",
            "/compat/linux/usr/bin:/compat/linux/bin:/usr/bin:/bin",
        )
        .status()
        .expect("compile signalfd shim test");
    assert!(compile.success());

    let run = Command::new(&output)
        .status()
        .expect("run signalfd shim test");
    assert!(run.success());
}

#[test]
fn flatpak_spawn_wrapper_preserves_arguments_and_injects_the_shim() {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output_dir = project_root.join("target/signalfd-tests");
    fs::create_dir_all(&output_dir).unwrap();
    let target = output_dir.join("flatpak-spawn-wrapper-target");
    let wrapper = output_dir.join("flatpak-spawn-wrapper");
    let shim = output_dir.join("libtest-preload.so");
    let linux_path = "/compat/linux/usr/bin:/compat/linux/bin:/usr/bin:/bin";

    let compile_target = Command::new("/compat/linux/usr/bin/gcc")
        .args(["-O2", "-Wall", "-Wextra", "-Werror"])
        .arg(project_root.join("tests/flatpak-spawn-wrapper-target.c"))
        .arg("-o")
        .arg(&target)
        .env("PATH", linux_path)
        .status()
        .expect("compile flatpak-spawn wrapper target");
    assert!(compile_target.success());

    let compile_shim = Command::new("/compat/linux/usr/bin/gcc")
        .args(["-shared", "-fPIC", "-x", "c", "/dev/null", "-o"])
        .arg(&shim)
        .env("PATH", linux_path)
        .status()
        .expect("compile test preload");
    assert!(compile_shim.success());

    let real_spawn = format!("-DREAL_FLATPAK_SPAWN=\"{}\"", target.display());
    let shim_define = format!("-DSIGNALFD_PRELOAD=\"{}\"", shim.display());
    let compile_wrapper = Command::new("/compat/linux/usr/bin/gcc")
        .args(["-O2", "-Wall", "-Wextra", "-Werror"])
        .arg(real_spawn)
        .arg(shim_define)
        .arg(project_root.join("compatibility_helpers/flatpak-spawn-wrapper.c"))
        .arg("-o")
        .arg(&wrapper)
        .env("PATH", linux_path)
        .status()
        .expect("compile flatpak-spawn wrapper");
    assert!(compile_wrapper.success());

    let run = Command::new(&wrapper)
        .args(["first", "second"])
        .env("LD_PRELOAD", &shim)
        .env(
            "EXPECTED_PRELOAD",
            format!("{}:{}", shim.display(), shim.display()),
        )
        .status()
        .expect("run flatpak-spawn wrapper");
    assert!(run.success());
}
