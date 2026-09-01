use super::*;
use crate::installation::application_records::write_app;
use crate::installation::installation_paths::Installation;
use crate::installation::{
    ensure_layout, get_app, reconcile_runtime_bindings, remove_run_record, write_pinned_run_record,
    write_pinned_run_record_with_extension_deployments, write_runtime, AppRecord, RuntimeRecord,
};
use std::fs;
use std::path::Path;

fn checkout(path: &Path, ref_name: &str, commit: &str) {
    fs::create_dir_all(path.join("files")).unwrap();
    fs::write(
        path.join("metadata"),
        "[Application]\nname=org.example.App\n",
    )
    .unwrap();
    fs::write(
        path.join(".ostree-commit"),
        format!("{ref_name}\n{commit}\n0\nflathub\n"),
    )
    .unwrap();
}

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
fn multiple_pinned_generations_retire_after_their_last_run() {
    let temp = std::env::temp_dir().join(format!(
        "freebsd-flatpak-state-generations-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&temp);
    let paths = Installation::for_test(&temp);
    ensure_layout(&paths).unwrap();
    let persistent = paths.app_data("org.example.App").unwrap().join("sentinel");
    fs::create_dir_all(persistent.parent().unwrap()).unwrap();
    fs::write(&persistent, "keep").unwrap();

    let first = app(&paths, "app-a", "runtime-1");
    let second = app(&paths, "app-b", "runtime-2");
    let current = app(&paths, "app-c", "runtime-3");
    for deployment in [&first, &second, &current] {
        checkout(
            &absolute(&paths, &deployment.app_dir),
            &deployment.app_ref,
            &deployment.app_commit,
        );
        checkout(
            &absolute(&paths, &deployment.runtime_dir),
            &format!("runtime/{}", deployment.runtime_ref),
            &deployment.runtime_commit,
        );
    }
    write_runtime(
        &paths,
        &RuntimeRecord {
            origin: "flathub".to_string(),
            runtime_ref: current.runtime_ref.clone(),
            runtime_commit: current.runtime_commit.clone(),
            explicitly_installed: false,
            installed_size: 0,
            runtime_dir: current.runtime_dir.clone(),
        },
    )
    .unwrap();
    write_app(&paths, &current).unwrap();
    let first_run = write_pinned_run_record(
        &paths,
        "first",
        &paths.chroots().join("first"),
        100,
        101,
        &first,
    )
    .unwrap();
    let second_run = write_pinned_run_record(
        &paths,
        "second",
        &paths.chroots().join("second"),
        200,
        201,
        &second,
    )
    .unwrap();

    assert!(cleanup_retired_deployments(&paths).unwrap().is_empty());
    remove_run_record(&first_run).unwrap();
    cleanup_retired_deployments(&paths).unwrap();
    assert!(!absolute(&paths, &first.app_dir).exists());
    assert!(!absolute(&paths, &first.runtime_dir).exists());
    assert!(absolute(&paths, &second.app_dir).exists());
    assert!(absolute(&paths, &second.runtime_dir).exists());

    remove_run_record(&second_run).unwrap();
    cleanup_retired_deployments(&paths).unwrap();
    assert!(!absolute(&paths, &second.app_dir).exists());
    assert!(!absolute(&paths, &second.runtime_dir).exists());
    assert!(absolute(&paths, &current.app_dir).exists());
    assert!(absolute(&paths, &current.runtime_dir).exists());
    assert_eq!(fs::read_to_string(&persistent).unwrap(), "keep");
    let _ = fs::remove_dir_all(&temp);
}

#[test]
fn recovery_reclaims_generations_after_stale_pin_is_removed() {
    let temp = std::env::temp_dir().join(format!(
        "freebsd-flatpak-state-recovery-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&temp);
    let paths = Installation::for_test(&temp);
    ensure_layout(&paths).unwrap();
    let retired = app(&paths, "app-old", "runtime-old");
    checkout(
        &absolute(&paths, &retired.app_dir),
        &retired.app_ref,
        &retired.app_commit,
    );
    checkout(
        &absolute(&paths, &retired.runtime_dir),
        &format!("runtime/{}", retired.runtime_ref),
        &retired.runtime_commit,
    );
    let run = write_pinned_run_record(
        &paths,
        "crashed",
        &paths.chroots().join("crashed"),
        i32::MAX as u32,
        0,
        &retired,
    )
    .unwrap();
    assert!(cleanup_retired_deployments(&paths).unwrap().is_empty());
    remove_run_record(&run).unwrap();
    assert_eq!(cleanup_retired_deployments(&paths).unwrap().len(), 2);
    let _ = fs::remove_dir_all(&temp);
}

#[test]
fn origin_transition_rebinds_apps_and_reclaims_the_stale_runtime_deployment() {
    let temp = std::env::temp_dir().join(format!(
        "freebsd-flatpak-runtime-origin-cleanup-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&temp);
    let paths = Installation::for_test(&temp);
    ensure_layout(&paths).unwrap();
    let mut installed_app = app(&paths, "app", "old-runtime");
    installed_app.runtime_origin = "old-origin".to_string();
    let old_dir = absolute(&paths, &installed_app.runtime_dir);
    let new_dir = old_dir.parent().unwrap().join("new-runtime");
    checkout(
        &old_dir,
        &format!("runtime/{}", installed_app.runtime_ref),
        "old-runtime",
    );
    checkout(
        &new_dir,
        &format!("runtime/{}", installed_app.runtime_ref),
        "new-runtime",
    );
    write_app(&paths, &installed_app).unwrap();
    write_runtime(
        &paths,
        &RuntimeRecord {
            origin: "new-origin".to_string(),
            runtime_ref: installed_app.runtime_ref.clone(),
            runtime_commit: "new-runtime".to_string(),
            explicitly_installed: false,
            installed_size: 0,
            runtime_dir: paths.relative_data_path(&new_dir).unwrap(),
        },
    )
    .unwrap();

    reconcile_runtime_bindings(&paths).unwrap();
    cleanup_retired_deployments(&paths).unwrap();
    let rebound = get_app(&paths, &installed_app.app_id).unwrap();
    assert_eq!(rebound.runtime_origin, "new-origin");
    assert_eq!(absolute(&paths, &rebound.runtime_dir), new_dir);
    assert!(!old_dir.exists());
    assert!(new_dir.exists());
    let _ = fs::remove_dir_all(&temp);
}

#[test]
fn running_app_pins_retired_extension_generation_until_exit() {
    let temp = std::env::temp_dir().join(format!(
        "freebsd-flatpak-extension-generation-{}",
        std::process::id()
    ));
    let paths = Installation::for_test(&temp);
    ensure_layout(&paths).unwrap();
    let old = paths.runtimes().join("org.example.Extension-x86_64-1/old");
    let current = old.parent().unwrap().join("current");
    checkout(&old, "runtime/org.example.Extension/x86_64/1", "old");
    checkout(
        &current,
        "runtime/org.example.Extension/x86_64/1",
        "current",
    );
    write_runtime(
        &paths,
        &RuntimeRecord {
            origin: "flathub".into(),
            runtime_ref: "org.example.Extension/x86_64/1".into(),
            runtime_commit: "current".into(),
            installed_size: 0,
            explicitly_installed: false,
            runtime_dir: paths.relative_data_path(&current).unwrap(),
        },
    )
    .unwrap();
    let deployment = app(&paths, "app", "runtime");
    let pin = write_pinned_run_record_with_extension_deployments(
        &paths,
        "running",
        &paths.chroots().join("running"),
        std::process::id(),
        0,
        &deployment,
        &["runtime/org.example.Extension/x86_64/1".to_string()],
        std::slice::from_ref(&old),
    )
    .unwrap();

    cleanup_retired_deployments(&paths).unwrap();
    assert!(old.exists());
    assert!(current.exists());
    remove_run_record(&pin).unwrap();
    cleanup_retired_deployments(&paths).unwrap();
    assert!(!old.exists());
    assert!(current.exists());
    let _ = fs::remove_dir_all(temp);
}
