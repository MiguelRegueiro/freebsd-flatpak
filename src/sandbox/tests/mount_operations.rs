use super::*;
use crate::sandbox::chroot_instance::OwnedMount;
use std::path::PathBuf;
#[test]
fn owned_mount_cleanup_is_child_first_and_root_scoped() {
    let root = PathBuf::from("/sandbox/first");
    let mounts = vec![
        OwnedMount {
            path: root.join("sys"),
            read_only: false,
        },
        OwnedMount {
            path: root.join("sys/class/drm"),
            read_only: true,
        },
        OwnedMount {
            path: root.join("dev"),
            read_only: false,
        },
        OwnedMount {
            path: root.join("dev/shm"),
            read_only: false,
        },
    ];

    let ordered = owned_mount_teardown_order(&root, mounts).unwrap();
    let positions = ordered
        .iter()
        .enumerate()
        .map(|(index, mount)| (mount.path.clone(), index))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert!(positions[&root.join("sys/class/drm")] < positions[&root.join("sys")]);
    assert!(positions[&root.join("dev/shm")] < positions[&root.join("dev")]);

    let other_instance_mount = OwnedMount {
        path: PathBuf::from("/sandbox/second/usr"),
        read_only: true,
    };
    assert!(owned_mount_teardown_order(&root, vec![other_instance_mount]).is_err());
}
