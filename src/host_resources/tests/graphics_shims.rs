use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn drm_syncobj_errno_shim_has_narrow_ioctl_behavior() {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output_dir = project_root.join("target/graphics-tests");
    fs::create_dir_all(&output_dir).unwrap();
    let output = output_dir.join("drm-syncobj-errno-shim-test");
    let compile = Command::new("/compat/linux/usr/bin/gcc")
        .args(["-O2", "-Wall", "-Wextra", "-Werror"])
        .arg("-DDRM_SYNCOBJ_ERRNO_SHIM_TEST")
        .arg(project_root.join("compatibility_helpers/drm-syncobj-errno-shim.c"))
        .arg(project_root.join("tests/drm-syncobj-errno-shim-test.c"))
        .arg("-o")
        .arg(&output)
        .env(
            "PATH",
            "/compat/linux/usr/bin:/compat/linux/bin:/usr/bin:/bin",
        )
        .status()
        .expect("compile DRM syncobj errno shim test");
    assert!(compile.success());

    let run = Command::new(&output)
        .status()
        .expect("run DRM syncobj errno shim test");
    assert!(run.success());
}

#[test]
fn chromium_zygote_preload_matches_only_zygote_exec() {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output_dir = project_root.join("target/graphics-tests");
    fs::create_dir_all(&output_dir).unwrap();
    let output = output_dir.join("chromium-zygote-drm-preload-test");
    let compile = Command::new("/compat/linux/usr/bin/gcc")
        .args(["-O2", "-Wall", "-Wextra", "-Werror"])
        .arg("-DCHROMIUM_ZYGOTE_DRM_PRELOAD_TEST")
        .arg(project_root.join("compatibility_helpers/chromium-zygote-drm-preload.c"))
        .arg(project_root.join("tests/chromium-zygote-drm-preload-test.c"))
        .arg("-o")
        .arg(&output)
        .env(
            "PATH",
            "/compat/linux/usr/bin:/compat/linux/bin:/usr/bin:/bin",
        )
        .status()
        .expect("compile Chromium zygote preload test");
    assert!(compile.success());

    let run = Command::new(&output)
        .status()
        .expect("run Zypak child spawn preload test");
    assert!(run.success());
}
