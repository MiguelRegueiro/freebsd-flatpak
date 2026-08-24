use crate::installation::application_records::write_app;
use crate::installation::installation_paths::Installation;
use crate::installation::{
    absolute, ensure_layout, get_app, reconcile_runtime_bindings, write_runtime, AppRecord,
    RuntimeRecord,
};
use std::fs;

fn app(paths: &Installation, app_commit: &str, runtime_commit: &str) -> AppRecord {
    AppRecord {
        origin: "flathub".to_string(),
        runtime_origin: "flathub".to_string(),
        app_id: "org.example.App".to_string(),
        app_ref: "app/org.example.App/x86_64/stable".to_string(),
        app_commit: app_commit.to_string(),
        app_dir: paths
            .relative_data_path(&paths.app("org.example.App").join(app_commit))
            .unwrap(),
        arch: "x86_64".to_string(),
        branch: "stable".to_string(),
        runtime_ref: "org.example.Platform/x86_64/stable".to_string(),
        runtime_commit: runtime_commit.to_string(),
        runtime_dir: paths
            .relative_data_path(
                &paths
                    .runtimes()
                    .join("org.example.Platform-stable")
                    .join(runtime_commit),
            )
            .unwrap(),
        command: "example".to_string(),
    }
}

#[test]
fn shared_runtime_activation_updates_every_future_launch_record() {
    let temp = std::env::temp_dir().join(format!(
        "freebsd-flatpak-state-shared-runtime-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&temp);
    let paths = Installation::for_test(&temp);
    ensure_layout(&paths).unwrap();
    let mut first = app(&paths, "app-one", "runtime-1");
    let mut second = app(&paths, "app-two", "runtime-1");
    second.app_id = "org.example.Other".to_string();
    second.app_ref = "app/org.example.Other/x86_64/stable".to_string();
    second.app_dir = paths
        .relative_data_path(&paths.app("org.example.Other").join("app-two"))
        .unwrap();
    write_app(&paths, &first).unwrap();
    write_app(&paths, &second).unwrap();
    let new_runtime_dir = paths
        .runtimes()
        .join("org.example.Platform-stable/runtime-2");
    write_runtime(
        &paths,
        &RuntimeRecord {
            origin: "flathub".to_string(),
            runtime_ref: first.runtime_ref.clone(),
            runtime_commit: "runtime-2".to_string(),
            runtime_dir: paths.relative_data_path(&new_runtime_dir).unwrap(),
        },
    )
    .unwrap();

    reconcile_runtime_bindings(&paths).unwrap();
    first = get_app(&paths, &first.app_id).unwrap();
    second = get_app(&paths, &second.app_id).unwrap();
    assert_eq!(first.runtime_commit, "runtime-2");
    assert_eq!(second.runtime_commit, "runtime-2");
    assert_eq!(absolute(&paths, &first.runtime_dir), new_runtime_dir);
    assert_eq!(first.runtime_dir, second.runtime_dir);
    let _ = fs::remove_dir_all(&temp);
}
