use super::*;
use crate::installation::application_records::write_app;
use crate::installation::installation_paths::Installation;
use crate::installation::{
    ensure_layout, remove_run_record, write_pinned_run_record, write_runtime, AppRecord,
    RuntimeRecord,
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
        format!("{ref_name}\n{commit}\n"),
    )
    .unwrap();
}

fn app(paths: &Installation, app_commit: &str, runtime_commit: &str) -> AppRecord {
    AppRecord {
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
            runtime_ref: current.runtime_ref.clone(),
            runtime_commit: current.runtime_commit.clone(),
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
