use super::{export_app, remove_export};
use crate::installation::installation_paths::Installation;
use crate::installation::AppRecord;
use std::fs;
use std::path::PathBuf;
#[test]
fn publishes_desktop_files_into_normal_xdg_data_home() {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-desktop-test-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let paths = Installation::for_test(&root);
    paths.ensure().unwrap();
    let source = paths
        .app("org.example.App")
        .join("export/share/applications/org.example.App.desktop");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(
        &source,
        "[Desktop Entry]\nName=Example\nExec=example %U\nDBusActivatable=true\n",
    )
    .unwrap();
    let app = AppRecord {
        origin: "flathub".to_string(),
        runtime_origin: "flathub".to_string(),
        app_id: "org.example.App".into(),
        app_ref: "app/org.example.App/x86_64/stable".into(),
        app_commit: "a".repeat(64),
        app_dir: PathBuf::from("apps/org.example.App"),
        arch: "x86_64".into(),
        branch: "stable".into(),
        runtime_ref: "org.example.Platform/x86_64/stable".into(),
        runtime_commit: "b".repeat(64),
        runtime_dir: PathBuf::from("runtimes/org.example.Platform-stable"),
        command: "example".into(),
    };

    let projected = paths
        .data_home()
        .join("applications/org.example.App.desktop");
    fs::create_dir_all(projected.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(
        "/tmp/deleted-benchmark/data/exports/share/applications/org.example.App.desktop",
        &projected,
    )
    .unwrap();

    export_app(&paths, &app).unwrap();
    assert!(fs::symlink_metadata(&projected)
        .unwrap()
        .file_type()
        .is_symlink());
    let desktop = fs::read_to_string(&projected).unwrap();
    assert!(desktop.contains("Exec=/usr/local/bin/flatpak run org.example.App -- %U"));
    assert!(!desktop.contains(root.to_str().unwrap()));

    remove_export(&paths, &app.app_id).unwrap();
    assert!(!projected.exists());
    let _ = fs::remove_dir_all(root);
}
