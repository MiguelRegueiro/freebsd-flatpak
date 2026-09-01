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
        installed_size: 0,
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
            explicitly_installed: false,
            installed_size: 0,
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

#[test]
fn same_ref_from_a_new_origin_replaces_provenance() {
    let temp = std::env::temp_dir().join(format!(
        "freebsd-flatpak-runtime-identity-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&temp);
    let paths = Installation::for_test(&temp);
    ensure_layout(&paths).unwrap();
    let runtime_ref = "org.example.Platform/x86_64/50";
    for origin in ["primary", "secondary"] {
        write_runtime(
            &paths,
            &RuntimeRecord {
                origin: origin.to_string(),
                runtime_ref: runtime_ref.to_string(),
                runtime_commit: format!("{origin}-commit"),
                explicitly_installed: true,
                installed_size: 42,
                runtime_dir: std::path::PathBuf::from(format!("runtimes/{origin}/runtime")),
            },
        )
        .unwrap();
    }

    let runtimes = super::list_runtimes(&paths).unwrap();
    assert_eq!(runtimes.len(), 1);
    assert_eq!(runtimes[0].origin, "secondary");
    assert_eq!(runtimes[0].runtime_commit, "secondary-commit");
    super::remove_runtime_record(&paths, runtime_ref).unwrap();
    assert!(super::get_runtime(&paths, runtime_ref).unwrap().is_none());
    let _ = fs::remove_dir_all(&temp);
}

#[test]
fn runtime_origin_transition_rebinds_every_dependent_app() {
    let temp = std::env::temp_dir().join(format!(
        "freebsd-flatpak-runtime-origin-transition-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&temp);
    let paths = Installation::for_test(&temp);
    ensure_layout(&paths).unwrap();
    let mut installed_app = app(&paths, "app", "old-runtime");
    installed_app.runtime_origin = "old-origin".to_string();
    write_app(&paths, &installed_app).unwrap();
    let runtime_dir = paths.runtimes().join("new-runtime");
    write_runtime(
        &paths,
        &RuntimeRecord {
            origin: "new-origin".to_string(),
            runtime_ref: installed_app.runtime_ref.clone(),
            runtime_commit: "new-runtime".to_string(),
            explicitly_installed: false,
            installed_size: 42,
            runtime_dir: paths.relative_data_path(&runtime_dir).unwrap(),
        },
    )
    .unwrap();

    reconcile_runtime_bindings(&paths).unwrap();
    let rebound = get_app(&paths, &installed_app.app_id).unwrap();
    assert_eq!(rebound.runtime_origin, "new-origin");
    assert_eq!(rebound.runtime_commit, "new-runtime");
    assert_eq!(absolute(&paths, &rebound.runtime_dir), runtime_dir);
    let _ = fs::remove_dir_all(&temp);
}
