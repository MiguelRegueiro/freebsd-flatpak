use super::*;
use std::path::PathBuf;

#[test]
fn strips_absolute_video_mount_target() {
    let mount = VideoMount {
        host_path: PathBuf::from("/host"),
        sandbox_path: PathBuf::from("/usr/lib/x86_64-linux-gnu/dri/intel-vaapi-driver"),
    };
    assert_eq!(
        mount.sandbox_target_relative().unwrap(),
        PathBuf::from("usr/lib/x86_64-linux-gnu/dri/intel-vaapi-driver")
    );
}

#[test]
fn aarch64_vaapi_paths_follow_the_activated_extension_mountpoint() {
    let video = HostVideo {
        vaapi: Some(RuntimeVaapiExtension {
            ref_name: "runtime/org.freedesktop.Platform.VAAPI.Intel/aarch64/25.08".to_string(),
            checkout_dir: PathBuf::from("/extensions/vaapi"),
            runtime_mount_relative: PathBuf::from("lib/aarch64-linux-gnu/dri/intel-vaapi-driver"),
            ld_library_relative: None,
        }),
        warnings: Vec::new(),
    };

    assert_eq!(
        video.env()[0],
        (
            "LIBVA_DRIVERS_PATH".to_string(),
            "/usr/lib/aarch64-linux-gnu/dri/intel-vaapi-driver:/usr/lib/aarch64-linux-gnu/dri:/usr/lib/dri"
                .to_string()
        )
    );
}
