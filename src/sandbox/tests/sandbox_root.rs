use super::*;
use crate::extensions::activation::{ExtensionMount, ExtensionScope};
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

#[test]
fn root_contains_flatpak_identity_files_instead_of_runtime_accounts() {
    let dir = test_dir("identity-files");
    let root = dir.join("root");
    let runtime_etc = dir.join("runtime-files/etc");
    fs::create_dir_all(&runtime_etc).unwrap();
    fs::write(runtime_etc.join("passwd"), "runtime:x:0:0::/:/bin/sh\n").unwrap();
    fs::write(runtime_etc.join("group"), "runtime:x:0:\n").unwrap();
    let identity = SandboxIdentity::new(
        1001,
        1002,
        "example".to_string(),
        Some("media".to_string()),
        "Example User".to_string(),
        PathBuf::from("/home/example"),
    )
    .unwrap();

    prepare_root(&root, &identity, &runtime_etc, false).unwrap();

    assert_eq!(
        fs::read_to_string(root.join("etc/passwd")).unwrap(),
        identity.passwd_contents()
    );
    assert_eq!(
        fs::read_to_string(root.join("etc/group")).unwrap(),
        identity.group_contents()
    );
    assert_eq!(
        fs::metadata(root.join("etc/passwd"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
}

#[test]
fn flatpak_info_reports_mounted_app_and_runtime_extensions_by_commit() {
    let dir = test_dir("flatpak-info-extensions");
    let app = FlatpakApp {
        app_id: "org.example.App".to_string(),
        app_dir: dir.join("app"),
        runtime_ref: "org.example.Platform/x86_64/1".to_string(),
        runtime_dir: dir.join("runtime"),
        command: "example".to_string(),
        args: Vec::new(),
    };
    let mount = |name: &str, commit: &str, scope| ExtensionMount {
        name: name.to_string(),
        ref_name: format!("runtime/{name}/x86_64/1"),
        commit: commit.to_string(),
        checkout_dir: dir.join(name),
        target: PathBuf::from(match scope {
            ExtensionScope::App => "app/ext",
            ExtensionScope::Runtime => "usr/ext",
        }),
        add_ld_paths: Vec::new(),
        merge_dirs: Vec::new(),
        priority: 0,
        scope,
        conditions: Vec::new(),
    };
    let plan = ExtensionMountPlan {
        mounts: vec![
            mount("org.example.AppPlugin", "app-commit", ExtensionScope::App),
            mount(
                "org.example.RuntimePlugin",
                "runtime-commit",
                ExtensionScope::Runtime,
            ),
        ],
    };
    write_flatpak_info(&dir, &app, "instance", &plan).unwrap();
    let info = fs::read_to_string(dir.join(".flatpak-info")).unwrap();
    assert!(info.contains("app-extensions=org.example.AppPlugin=app-commit;"));
    assert!(info.contains("runtime-extensions=org.example.RuntimePlugin=runtime-commit;"));
}
