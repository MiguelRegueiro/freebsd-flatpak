use super::*;
use crate::extensions::activation::{ExtensionMount, ExtensionScope};
use std::path::PathBuf;

#[test]
fn aarch64_vaapi_paths_follow_the_activated_extension_mountpoint() {
    let video = HostVideo {
        vaapi: Some(ExtensionMount {
            name: "org.example.Video.Intel".to_string(),
            ref_name: "runtime/org.freedesktop.Platform.VAAPI.Intel/aarch64/25.08".to_string(),
            commit: "commit".to_string(),
            checkout_dir: PathBuf::from("/extensions/vaapi"),
            target: PathBuf::from("usr/lib/aarch64-linux-gnu/dri/intel-vaapi-driver"),
            add_ld_paths: Vec::new(),
            merge_dirs: Vec::new(),
            priority: 0,
            scope: ExtensionScope::Runtime,
            conditions: vec!["have-intel-gpu".to_string()],
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
