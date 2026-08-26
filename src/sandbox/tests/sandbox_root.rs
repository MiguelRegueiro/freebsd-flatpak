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

#[test]
fn prepared_root_has_private_tmp_and_legacy_flatpak_info_link() {
    let dir = test_dir("flatpak-root-layout");
    let root = dir.join("root");
    let runtime_etc = dir.join("runtime-etc");
    fs::create_dir_all(&runtime_etc).unwrap();

    prepare_root(
        &root,
        1001,
        1002,
        "example",
        Path::new("/var/home/example"),
        &runtime_etc,
        false,
    )
    .unwrap();

    assert!(root.join("tmp").is_dir());
    assert_eq!(
        fs::read_link(root.join("run/user/1001/flatpak-info")).unwrap(),
        PathBuf::from("../../../.flatpak-info")
    );
    assert!(fs::read_to_string(root.join("etc/passwd"))
        .unwrap()
        .contains("example:x:1001:1002:example:/var/home/example:/bin/sh"));
    assert!(fs::read_to_string(root.join("etc/group"))
        .unwrap()
        .contains("example:x:1002:example"));
}

#[test]
fn flatpak_info_describes_the_pinned_generic_application_instance() {
    let dir = test_dir("flatpak-info");
    let root = dir.join("root");
    let app_dir = dir.join("app-checkout");
    let runtime_dir = dir.join("runtime-checkout");
    let instance_path = dir.join("home/.var/app/org.example.App");
    let app_extension = dir.join("app-extension");
    let runtime_extension = dir.join("runtime-extension");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&app_extension).unwrap();
    fs::create_dir_all(&runtime_extension).unwrap();
    fs::write(
        app_extension.join(".ostree-commit"),
        "runtime/org.example.App.Extension/x86_64/stable\napp-extension-commit\n1\napps\n",
    )
    .unwrap();
    fs::write(
        runtime_extension.join(".ostree-commit"),
        "runtime/org.example.Platform.Extension/x86_64/stable\nruntime-extension-commit\n1\nruntimes\n",
    )
    .unwrap();
    let app = FlatpakApp {
        app_id: "org.example.App".into(),
        app_dir: app_dir.clone(),
        runtime_ref: "org.example.Platform/x86_64/stable".into(),
        runtime_dir: runtime_dir.clone(),
        command: "example".into(),
        args: Vec::new(),
    };
    let deployment = crate::installation::AppRecord {
        origin: "apps".into(),
        runtime_origin: "runtimes".into(),
        app_id: app.app_id.clone(),
        app_ref: "app/org.example.App/x86_64/stable".into(),
        app_commit: "app-commit".into(),
        installed_size: 1,
        app_dir: app_dir.clone(),
        arch: "x86_64".into(),
        branch: "stable".into(),
        runtime_ref: app.runtime_ref.clone(),
        runtime_commit: "runtime-commit".into(),
        runtime_dir: runtime_dir.clone(),
        command: app.command.clone(),
    };

    write_flatpak_info(
        &root,
        &app,
        &deployment,
        "instance-123",
        &instance_path,
        &[FlatpakInfoExtension {
            ref_name: "runtime/org.example.App.Extension/x86_64/stable",
            checkout_dir: &app_extension,
        }],
        &[FlatpakInfoExtension {
            ref_name: "runtime/org.example.Platform.Extension/x86_64/stable",
            checkout_dir: &runtime_extension,
        }],
    )
    .unwrap();

    let info = glib::KeyFile::new();
    info.load_from_file(root.join(".flatpak-info"), glib::KeyFileFlags::NONE)
        .unwrap();
    assert_eq!(info.string("Application", "name").unwrap(), app.app_id);
    assert_eq!(
        info.string("Application", "runtime").unwrap(),
        format!("runtime/{}", app.runtime_ref)
    );
    assert_eq!(
        info.string("Instance", "instance-id").unwrap(),
        "instance-123"
    );
    assert_eq!(
        info.string("Instance", "instance-path").unwrap(),
        instance_path.to_string_lossy().as_ref()
    );
    assert_eq!(
        info.string("Instance", "app-path").unwrap(),
        app_dir.join("files").to_string_lossy().as_ref()
    );
    assert_eq!(
        info.string("Instance", "runtime-path").unwrap(),
        runtime_dir.join("files").to_string_lossy().as_ref()
    );
    assert_eq!(info.string("Instance", "app-commit").unwrap(), "app-commit");
    assert_eq!(
        info.string("Instance", "runtime-commit").unwrap(),
        "runtime-commit"
    );
    assert_eq!(
        info.string_list("Instance", "app-extensions").unwrap()[0],
        "org.example.App.Extension=app-extension-commit"
    );
    assert_eq!(
        info.string_list("Instance", "runtime-extensions").unwrap()[0],
        "org.example.Platform.Extension=runtime-extension-commit"
    );
    assert_eq!(info.string("Instance", "branch").unwrap(), "stable");
    assert_eq!(info.string("Instance", "arch").unwrap(), "x86_64");
    assert_eq!(
        info.string("Instance", "flatpak-version").unwrap(),
        crate::flatpak_compatibility::FLATPAK_COMPATIBILITY_VERSION
    );
    assert!(info.boolean("Instance", "session-bus-proxy").unwrap());
    assert!(info.boolean("Instance", "system-bus-proxy").unwrap());
    assert_eq!(info.string("Context", "filesystems").unwrap(), "");
}
