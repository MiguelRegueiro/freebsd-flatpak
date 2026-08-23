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
