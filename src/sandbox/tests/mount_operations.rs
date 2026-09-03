use super::*;
use crate::sandbox::chroot_instance::OwnedMount;
use std::path::PathBuf;
#[test]
fn complete_single_source_merge_uses_one_directory_mount() {
    use crate::extensions::activation::{ExtensionMergeDirectory, ExtensionMergeEntry};
    use std::fs;

    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-direct-merge-{}",
        std::process::id()
    ));
    let base = root.join("base");
    let source = root.join("source");
    fs::create_dir_all(&base).unwrap();
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("driver-a"), "a").unwrap();
    fs::write(source.join("driver-b"), "b").unwrap();
    let merge = ExtensionMergeDirectory {
        target: PathBuf::from("usr/lib/GL/lib/dri"),
        base_source: base.clone(),
        entries: ["driver-a", "driver-b"]
            .into_iter()
            .map(|name| ExtensionMergeEntry {
                name: PathBuf::from(name),
                source: source.join(name),
            })
            .collect(),
    };

    assert_eq!(direct_merge_source(&merge), Some(source.clone()));
    fs::write(base.join("runtime-driver"), "base").unwrap();
    assert_eq!(direct_merge_source(&merge), None);
    fs::remove_file(base.join("runtime-driver")).unwrap();
    let mut incomplete = merge.clone();
    incomplete.entries.pop();
    assert_eq!(direct_merge_source(&incomplete), None);
    fs::remove_dir_all(root).unwrap();
}

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

#[test]
fn nested_and_symlinked_sources_are_compared_by_actual_topology() {
    use crate::sandbox::chroot_instance::NullfsMapping;
    use std::fs;
    use std::os::unix::fs::symlink;

    let base = std::env::temp_dir().join(format!(
        "freebsd-flatpak-nullfs-alias-{}",
        std::process::id()
    ));
    let host = base.join("host");
    let sandbox = base.join("sandbox");
    fs::create_dir_all(host.join("app/.local/share/Steam")).unwrap();
    fs::create_dir_all(host.join("app/ordinary/deep")).unwrap();
    fs::create_dir_all(&sandbox).unwrap();
    symlink(".local/share", host.join("app/data")).unwrap();

    let mappings = vec![NullfsMapping {
        source: host.join("app"),
        target: sandbox.join("app"),
    }];
    let mounts = vec![OwnedMount {
        path: sandbox.join("app"),
        read_only: false,
    }];
    for (source, target) in [
        (
            fs::canonicalize(host.join("app/ordinary")).unwrap(),
            sandbox.join("app/ordinary"),
        ),
        (
            fs::canonicalize(host.join("app/ordinary/deep")).unwrap(),
            sandbox.join("app/ordinary/deep"),
        ),
        (
            fs::canonicalize(host.join("app/data/Steam")).unwrap(),
            sandbox.join("app/data/Steam"),
        ),
    ] {
        assert!(nullfs_source_aliases_parent(
            &mappings, &mounts, &source, &target
        ));
    }
    assert!(!nullfs_source_aliases_parent(
        &mappings,
        &mounts,
        &fs::canonicalize(host.join("app/ordinary")).unwrap(),
        &sandbox.join("app/unrelated")
    ));

    let masked_mounts = vec![
        mounts[0].clone(),
        OwnedMount {
            path: sandbox.join("app/data"),
            read_only: false,
        },
    ];
    assert!(!nullfs_source_aliases_parent(
        &mappings,
        &masked_mounts,
        &fs::canonicalize(host.join("app/data/Steam")).unwrap(),
        &sandbox.join("app/data/Steam")
    ));

    fs::remove_dir_all(base).unwrap();
}
