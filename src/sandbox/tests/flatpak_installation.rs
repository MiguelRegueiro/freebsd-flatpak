use super::*;
use crate::installation::{self as state, AppRecord};

#[test]
fn projection_publishes_active_flatpak_layout_without_touching_data_home() {
    let root =
        std::env::temp_dir().join(format!("freebsd-flatpak-projection-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let paths = Installation::for_test(&root);
    let checkout = paths.app("org.example.App").join("commit");
    fs::create_dir_all(checkout.join("files")).unwrap();
    state::write_app(
        &paths,
        &AppRecord {
            origin: "flathub".into(),
            runtime_origin: "flathub".into(),
            app_id: "org.example.App".into(),
            app_ref: "app/org.example.App/x86_64/stable".into(),
            app_commit: "commit".into(),
            installed_size: 1,
            app_dir: paths.relative_data_path(&checkout).unwrap(),
            arch: "x86_64".into(),
            branch: "stable".into(),
            runtime_ref: "org.example.Platform/x86_64/stable".into(),
            runtime_commit: "runtime".into(),
            runtime_dir: PathBuf::from("runtimes/runtime"),
            command: "example".into(),
        },
    )
    .unwrap();

    let sandbox = root.join("sandbox");
    let projection = FlatpakInstallationProjection::prepare(&sandbox, &paths).unwrap();
    let expected_root = paths.data_home().join("flatpak/app");
    assert_eq!(
        projection.target_root,
        expected_root.strip_prefix("/").unwrap()
    );
    assert_eq!(projection.deployments.len(), 1);
    assert_eq!(projection.deployments[0].source, checkout);
    assert_eq!(
        projection.deployments[0].target,
        projection
            .target_root
            .join("org.example.App/current/active")
    );
    assert!(projection
        .source_root
        .join("org.example.App/current/active")
        .is_dir());
    assert!(!expected_root.exists());
    let _ = fs::remove_dir_all(root);
}
