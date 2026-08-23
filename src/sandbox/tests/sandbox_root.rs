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
    let dir = test_dir("network-metadata");
    let metadata = dir.join("metadata");
    fs::write(&metadata, "[Context]\nshared=ipc;network;\n").unwrap();

    assert!(app_allows_network(&metadata).unwrap());
}

#[test]
fn missing_network_shared_permission_disables_resolver_overlay() {
    let dir = test_dir("non-network-metadata");
    let metadata = dir.join("metadata");
    fs::write(&metadata, "[Context]\nshared=ipc;\n").unwrap();

    assert!(!app_allows_network(&metadata).unwrap());
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
