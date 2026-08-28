use super::*;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static TEST_ID: AtomicUsize = AtomicUsize::new(0);
fn test_dir(name: &str) -> PathBuf {
    let id = TEST_ID.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "freebsd-flatpak-root-test-{name}-{}-{id}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn shared_network_enables_resolver_overlay() {
    assert!(app_allows_network("[Context]\nshared=ipc;network;\n"));
}

#[test]
fn missing_network_shared_permission_disables_resolver_overlay() {
    assert!(!app_allows_network("[Context]\nshared=ipc;\n"));
}

#[test]
fn etc_overlay_preserves_runtime_etc_and_adds_network_resolver_files() {
    let dir = test_dir("etc-overlay");
    let root = dir.join("root");
    let runtime_etc = dir.join("runtime-etc");
    fs::create_dir_all(&runtime_etc).unwrap();
    fs::write(runtime_etc.join("nsswitch.conf"), "hosts: files dns\n").unwrap();

    prepare_etc_overlay(&root, &runtime_etc, true).unwrap();

    assert!(root.join("etc/resolv.conf").is_file());
    assert!(root.join("etc/hosts").is_file());
    assert_eq!(
        fs::read_link(root.join("etc/nsswitch.conf")).unwrap(),
        PathBuf::from("/usr/etc/nsswitch.conf")
    );
}

#[test]
fn etc_overlay_exposes_runtime_os_release_with_flatpak_layout() {
    let dir = test_dir("runtime-os-release");
    let root = dir.join("root");
    let runtime_files = dir.join("runtime-files");
    let runtime_etc = runtime_files.join("etc");
    fs::create_dir_all(runtime_files.join("lib")).unwrap();
    fs::create_dir_all(&runtime_etc).unwrap();
    fs::write(runtime_files.join("lib/os-release"), "NAME=Runtime\n").unwrap();
    std::os::unix::fs::symlink("../usr/lib/os-release", runtime_etc.join("os-release")).unwrap();

    prepare_etc_overlay(&root, &runtime_etc, false).unwrap();

    assert_eq!(
        fs::read_link(root.join("etc/os-release")).unwrap(),
        PathBuf::from("../usr/lib/os-release")
    );
}

#[test]
fn etc_overlay_does_not_fabricate_missing_runtime_os_release() {
    let dir = test_dir("missing-runtime-os-release");
    let root = dir.join("root");
    let runtime_etc = dir.join("runtime-files/etc");
    fs::create_dir_all(&runtime_etc).unwrap();
    std::os::unix::fs::symlink("../usr/lib/os-release", runtime_etc.join("os-release")).unwrap();

    prepare_etc_overlay(&root, &runtime_etc, false).unwrap();

    assert!(fs::symlink_metadata(root.join("etc/os-release")).is_err());
}
