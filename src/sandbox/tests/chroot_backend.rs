use super::*;
use crate::sandbox::stale_sandbox_recovery::{chroot_root_for_mount, remove_instance_root};
use std::sync::atomic::{AtomicUsize, Ordering};

static TEST_ID: AtomicUsize = AtomicUsize::new(0);

fn test_dir(name: &str) -> PathBuf {
    let id = TEST_ID.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "freebsd-flatpak-sandbox-{name}-{}-{id}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn concurrent_instances_get_distinct_roots_and_cleanup_isolation() {
    let dir = test_dir("concurrent-instance-roots");
    let app_root = dir.join("chroots").join("org.example.App");
    let first_id = new_instance_id();
    let second_id = new_instance_id();
    let first_root = app_root.join(&first_id);
    let second_root = app_root.join(&second_id);
    fs::create_dir_all(first_root.join("usr")).unwrap();
    fs::create_dir_all(second_root.join("usr")).unwrap();
    fs::write(first_root.join(".flatpak-info"), "first").unwrap();
    fs::write(second_root.join(".flatpak-info"), "second").unwrap();

    assert_ne!(first_id, second_id);
    assert_eq!(
        chroot_root_for_mount(&dir.join("chroots"), &first_root.join("usr")),
        Some(first_root.clone())
    );
    assert_eq!(
        chroot_root_for_mount(&dir.join("chroots"), &second_root.join("usr")),
        Some(second_root.clone())
    );

    remove_instance_root(&first_root).unwrap();
    assert!(!first_root.exists());
    assert!(second_root.join(".flatpak-info").is_file());
}

#[test]
fn persistent_directory_creates_canonical_path_for_new_data() {
    let dir = test_dir("persistent-canonical");
    let app_data = dir.join("app-data");
    fs::create_dir_all(&app_data).unwrap();
    let expected = app_data.join(".example/profile");

    assert_eq!(
        ensure_persistent_directory(&app_data, Path::new(".example/profile")).unwrap(),
        expected
    );
    assert!(expected.is_dir());
}

#[test]
fn x11_socket_access_follows_flatpak_metadata() {
    let dir = test_dir("x11-socket-metadata");
    let metadata = dir.join("metadata");

    fs::write(&metadata, "[Context]\nsockets=wayland;x11;pulseaudio;\n").unwrap();
    assert!(app_requests_socket(&metadata, "x11").unwrap());

    fs::write(&metadata, "[Context]\nsockets=wayland;pulseaudio;\n").unwrap();
    assert!(!app_requests_socket(&metadata, "x11").unwrap());
}
