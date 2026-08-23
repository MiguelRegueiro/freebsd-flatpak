use super::HostGraphics;
use crate::host_resources::graphics_shims::{DrmSyncobjErrnoShim, WaylandDrmDevtShim};
use std::path::PathBuf;

#[test]
fn keeps_drm_and_wayland_preloads_separate_on_one_mount() {
    let host_dir = PathBuf::from("/tmp/freebsd-flatpak-poc-graphics-shims");
    let graphics = HostGraphics {
        gl: None,
        drm: None,
        drm_syncobj_errno_shim: Some(DrmSyncobjErrnoShim {
            host_dir: host_dir.clone(),
        }),
        chromium_zygote_drm_preload: None,
        wayland_drm_devt_shim: Some(WaylandDrmDevtShim {
            host_dir: host_dir.clone(),
            dev_t_map: "0x61=0xe200".to_string(),
        }),
        warnings: Vec::new(),
    };

    assert_eq!(
        graphics.ld_preload_paths(),
        vec![
            "/run/host/freebsd-flatpak-poc/libdrm-syncobj-errno-shim.so",
            "/run/host/freebsd-flatpak-poc/libwayland-drm-devt-shim.so",
        ]
    );
    assert_eq!(
        graphics.zypak_ld_preload_paths(),
        vec![
            "/run/host/freebsd-flatpak-poc/libwayland-drm-devt-shim.so",
            "/run/host/freebsd-flatpak-poc/libdrm-syncobj-errno-shim.so",
        ]
    );
    let mounts = graphics.runtime_mounts();
    assert_eq!(mounts.len(), 1);
    assert_eq!(mounts[0].host_path(), host_dir);
    assert_eq!(
        mounts[0].sandbox_target_relative().unwrap(),
        PathBuf::from("run/host/freebsd-flatpak-poc")
    );
}
