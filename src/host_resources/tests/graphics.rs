use super::HostGraphics;
use crate::architecture::FlatpakArchitecture;
use crate::extensions::activation::{ExtensionMount, ExtensionScope};
use crate::host_resources::graphics_shims::{
    DrmSyncobjErrnoShim, Gtk3WaylandGeometryShim, WaylandDrmDevtShim,
};
use std::path::PathBuf;

#[test]
fn keeps_drm_and_wayland_preloads_separate_on_one_mount() {
    let host_dir = PathBuf::from("/tmp/freebsd-flatpak-graphics-shims");
    let graphics = HostGraphics {
        architecture: FlatpakArchitecture::X86_64,
        gl: None,
        drm: None,
        drm_syncobj_errno_shim: Some(DrmSyncobjErrnoShim {
            host_dir: host_dir.clone(),
        }),
        gtk3_wayland_geometry_shim: Some(Gtk3WaylandGeometryShim {
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
            "/run/host/freebsd-flatpak/libdrm-syncobj-errno-shim.so",
            "/run/host/freebsd-flatpak/libwayland-drm-devt-shim.so",
            "/run/host/freebsd-flatpak/libgtk3-wayland-geometry-shim.so",
        ]
    );
    assert_eq!(
        graphics.zypak_ld_preload_paths(),
        vec![
            "/run/host/freebsd-flatpak/libwayland-drm-devt-shim.so",
            "/run/host/freebsd-flatpak/libgtk3-wayland-geometry-shim.so",
            "/run/host/freebsd-flatpak/libdrm-syncobj-errno-shim.so",
        ]
    );
    let mounts = graphics.runtime_mounts();
    assert_eq!(mounts.len(), 1);
    assert_eq!(mounts[0].host_path(), host_dir);
    assert_eq!(
        mounts[0].sandbox_target_relative().unwrap(),
        PathBuf::from("run/host/freebsd-flatpak")
    );
}

#[test]
fn aarch64_gl_paths_follow_the_activated_extension_mountpoint() {
    let graphics = HostGraphics {
        architecture: FlatpakArchitecture::Aarch64,
        gl: Some(ExtensionMount {
            name: "org.example.Graphics.default".to_string(),
            ref_name: "runtime/org.freedesktop.Platform.GL.default/aarch64/25.08".to_string(),
            commit: "commit".to_string(),
            checkout_dir: PathBuf::from("/extensions/gl"),
            target: PathBuf::from("usr/lib/aarch64-linux-gnu/GL/default"),
            add_ld_paths: Vec::new(),
            merge_dirs: Vec::new(),
            priority: 0,
            scope: ExtensionScope::Runtime,
            conditions: vec!["active-gl-driver".to_string()],
        }),
        drm: None,
        drm_syncobj_errno_shim: None,
        gtk3_wayland_geometry_shim: None,
        chromium_zygote_drm_preload: None,
        wayland_drm_devt_shim: None,
        warnings: Vec::new(),
    };

    assert_eq!(
        graphics.ld_library_paths(),
        vec!["/usr/lib/aarch64-linux-gnu/GL/default/lib"]
    );
    let env = graphics.env();
    assert_eq!(
        env.iter()
            .find(|(key, _)| key == "LIBGL_DRIVERS_PATH")
            .map(|(_, value)| value.as_str()),
        Some("/usr/lib/aarch64-linux-gnu/GL/default/lib/dri")
    );
    assert_eq!(
        env.iter()
            .find(|(key, _)| key == "GBM_BACKENDS_PATH")
            .map(|(_, value)| value.as_str()),
        Some("/usr/lib/aarch64-linux-gnu/GL/default/lib/gbm")
    );
}
