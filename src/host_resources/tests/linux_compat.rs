use super::*;
use std::fs;
use std::process::Command;

#[test]
fn linux_compat_helpers_are_mounted_and_required() {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-linux-compat-test-{}",
        std::process::id()
    ));
    let libexec = root.join("libexec");
    fs::create_dir_all(&libexec).unwrap();
    fs::create_dir_all(libexec.join("linux-bin")).unwrap();
    fs::write(libexec.join(SIGNALFD_SHIM_LIB), []).unwrap();
    fs::write(libexec.join(SOCKET_OPTION_ERRNO_SHIM_LIB), []).unwrap();
    fs::write(libexec.join(FLATPAK_SPAWN_WRAPPER), []).unwrap();
    let paths = Installation::for_test(&root);

    let compatibility = HostLinuxCompat::prepare(&paths).unwrap();
    let (source, target) = compatibility.runtime_mount();
    assert_eq!(source, libexec);
    assert_eq!(target, PathBuf::from("run/host/freebsd-flatpak"));
    assert_eq!(
        compatibility.preload_paths(),
        vec!["/run/host/freebsd-flatpak/libsocket-option-errno-shim.so"]
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn runtime_flatpak_spawn_is_preserved_before_the_wrapper_is_projected() {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-linux-compat-projection-test-{}",
        std::process::id()
    ));
    let libexec = root.join("libexec");
    let runtime_files = root.join("runtime/files");
    let runtime_bin = runtime_files.join(RUNTIME_BIN);
    let sandbox_root = root.join("sandbox");
    fs::create_dir_all(libexec.join("linux-bin")).unwrap();
    fs::create_dir_all(&runtime_bin).unwrap();
    fs::write(libexec.join(SIGNALFD_SHIM_LIB), []).unwrap();
    fs::write(libexec.join(SOCKET_OPTION_ERRNO_SHIM_LIB), []).unwrap();
    fs::write(libexec.join(FLATPAK_SPAWN_WRAPPER), []).unwrap();
    fs::write(runtime_bin.join(RUNTIME_FLATPAK_SPAWN), []).unwrap();
    fs::write(runtime_bin.join("marker"), []).unwrap();
    let paths = Installation::for_test(&root);

    let compatibility = HostLinuxCompat::prepare(&paths).unwrap();
    let mounts = compatibility
        .prepare_runtime_binary_mounts(
            &sandbox_root,
            &runtime_files,
            Some("unix:path=/run/user/1001/private-bus"),
        )
        .unwrap();
    assert_eq!(mounts.len(), 3);
    assert_eq!(mounts[0].host_path(), runtime_bin);
    assert_eq!(
        mounts[0].sandbox_target_relative().unwrap(),
        PathBuf::from("run/freebsd-flatpak/runtime-bin")
    );
    assert_eq!(
        mounts[1].host_path(),
        sandbox_root.join(BIN_OVERLAY_RELATIVE)
    );
    assert_eq!(
        mounts[1].sandbox_target_relative().unwrap(),
        PathBuf::from("usr/bin")
    );
    assert_eq!(
        fs::read_link(sandbox_root.join(BIN_OVERLAY_RELATIVE).join("marker")).unwrap(),
        PathBuf::from("/run/freebsd-flatpak/runtime-bin/marker")
    );
    assert!(sandbox_root
        .join(BIN_OVERLAY_RELATIVE)
        .join(RUNTIME_FLATPAK_SPAWN)
        .is_file());
    assert_eq!(mounts[2].host_path(), libexec.join(FLATPAK_SPAWN_WRAPPER));
    assert_eq!(
        fs::read_link(sandbox_root.join(SESSION_BUS_RELATIVE)).unwrap(),
        PathBuf::from("/run/user/1001/private-bus")
    );
    assert_eq!(
        mounts[2].sandbox_target_relative().unwrap(),
        PathBuf::from("usr/bin/flatpak-spawn")
    );

    fs::remove_file(runtime_files.join(RUNTIME_BIN).join(RUNTIME_FLATPAK_SPAWN)).unwrap();
    assert!(compatibility
        .prepare_runtime_binary_mounts(&sandbox_root, &runtime_files, None)
        .unwrap()
        .is_empty());
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
    let session_bus_proxy = output_dir.join("session-bus-proxy");
    fs::write(&session_bus_proxy, []).unwrap();
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
    let bus_define = format!("-DSESSION_BUS_PROXY=\"{}\"", session_bus_proxy.display());
    let compile_wrapper = Command::new("/compat/linux/usr/bin/gcc")
        .args(["-O2", "-Wall", "-Wextra", "-Werror"])
        .arg(real_spawn)
        .arg(shim_define)
        .arg(bus_define)
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

    let clean_environment_run = Command::new(&wrapper)
        .args(["first", "second"])
        .env_clear()
        .env("EXPECTED_PRELOAD", &shim)
        .env(
            "EXPECTED_BUS",
            format!("unix:path={}", session_bus_proxy.display()),
        )
        .status()
        .expect("run flatpak-spawn wrapper with a clean environment");
    assert!(clean_environment_run.success());
}
