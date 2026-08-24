use super::{export_app, remove_export};
use crate::installation::installation_paths::Installation;
use crate::installation::AppRecord;
use std::fs;
use std::os::unix::fs as unix_fs;
use std::path::PathBuf;

fn app_record(app_id: &str) -> AppRecord {
    AppRecord {
        origin: "flathub".to_string(),
        runtime_origin: "flathub".to_string(),
        app_id: app_id.into(),
        app_ref: format!("app/{app_id}/x86_64/stable"),
        app_commit: "a".repeat(64),
        installed_size: 0,
        app_dir: PathBuf::from(format!("apps/{app_id}")),
        arch: "x86_64".into(),
        branch: "stable".into(),
        runtime_ref: "org.example.Platform/x86_64/stable".into(),
        runtime_commit: "b".repeat(64),
        runtime_dir: PathBuf::from("runtimes/org.example.Platform-stable"),
        command: "example".into(),
    }
}

fn write_desktop_export(paths: &Installation, app: &AppRecord) -> PathBuf {
    let source = paths
        .app(&app.app_id)
        .join(format!("export/share/applications/{}.desktop", app.app_id));
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(
        &source,
        "[Desktop Entry]\nName=Example\nExec=example %U\nDBusActivatable=true\n",
    )
    .unwrap();
    source
}

fn clean_test_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-desktop-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    root
}

#[test]
fn publishes_desktop_files_into_normal_xdg_data_home() {
    let root = clean_test_root("stale-managed-link");
    let paths = Installation::for_test(&root);
    paths.ensure().unwrap();
    let app = app_record("org.example.App");
    write_desktop_export(&paths, &app);

    let projected = paths
        .data_home()
        .join("applications/org.example.App.desktop");
    fs::create_dir_all(projected.parent().unwrap()).unwrap();
    unix_fs::symlink(
        "/tmp/deleted-benchmark/data/exports/share/applications/org.example.App.desktop",
        &projected,
    )
    .unwrap();

    let report = export_app(&paths, &app).unwrap();
    assert!(report.conflicts.is_empty());
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

#[test]
fn preserves_user_owned_symlink_that_conflicts_with_desktop_projection() {
    let root = clean_test_root("user-symlink-conflict");
    let paths = Installation::for_test(&root);
    paths.ensure().unwrap();
    let app = app_record("org.gtk.Demo4");
    write_desktop_export(&paths, &app);

    let override_file = root.join("desktop-overrides/org.gtk.Demo4.desktop");
    fs::create_dir_all(override_file.parent().unwrap()).unwrap();
    fs::write(
        &override_file,
        "[Desktop Entry]\nType=Application\nName=GTK Demo\nNoDisplay=true\nHidden=true\n",
    )
    .unwrap();
    let projected = paths.data_home().join("applications/org.gtk.Demo4.desktop");
    fs::create_dir_all(projected.parent().unwrap()).unwrap();
    unix_fs::symlink(&override_file, &projected).unwrap();

    let report = export_app(&paths, &app).unwrap();
    assert_eq!(
        report.conflicts,
        vec![PathBuf::from("applications/org.gtk.Demo4.desktop")]
    );
    assert_eq!(fs::read_link(&projected).unwrap(), override_file);
    assert!(fs::read_to_string(&projected)
        .unwrap()
        .contains("Hidden=true"));

    let private_export = paths
        .export_share()
        .join("applications/org.gtk.Demo4.desktop");
    assert!(fs::read_to_string(&private_export)
        .unwrap()
        .contains("Exec=/usr/local/bin/flatpak run org.gtk.Demo4 -- %U"));

    // Uninstall and post-record install rollback share remove_export(). Neither
    // may remove a projection unless it still points at the private export.
    remove_export(&paths, &app.app_id).unwrap();
    assert_eq!(fs::read_link(&projected).unwrap(), override_file);
    assert!(!private_export.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn preserves_user_owned_regular_file_that_conflicts_with_desktop_projection() {
    let root = clean_test_root("user-file-conflict");
    let paths = Installation::for_test(&root);
    paths.ensure().unwrap();
    let app = app_record("org.example.FileConflict");
    write_desktop_export(&paths, &app);

    let projected = paths
        .data_home()
        .join("applications/org.example.FileConflict.desktop");
    fs::create_dir_all(projected.parent().unwrap()).unwrap();
    let user_desktop = "[Desktop Entry]\nType=Application\nName=User owned\nExec=true\n";
    fs::write(&projected, user_desktop).unwrap();

    let report = export_app(&paths, &app).unwrap();
    assert_eq!(report.conflicts.len(), 1);
    assert_eq!(fs::read_to_string(&projected).unwrap(), user_desktop);

    remove_export(&paths, &app.app_id).unwrap();
    assert_eq!(fs::read_to_string(&projected).unwrap(), user_desktop);
    let _ = fs::remove_dir_all(root);
}
