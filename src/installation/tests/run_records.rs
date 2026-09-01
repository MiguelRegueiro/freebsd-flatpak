use super::*;
use crate::installation::installation_paths::Installation;
use crate::installation::{
    remove_run_record, write_checkout_pin, write_pinned_run_record_with_extension_deployments,
    write_run_record, AppRecord,
};
use std::fs;

#[test]
fn concurrent_run_records_are_distinct_and_cleanup_is_isolated() {
    let temp = std::env::temp_dir().join(format!(
        "freebsd-flatpak-state-concurrent-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&temp);
    let paths = Installation::for_test(&temp);
    let first_root = paths.chroots().join("org.example.App/first");
    let second_root = paths.chroots().join("org.example.App/second");

    let first =
        write_run_record(&paths, "org.example.App", "first", &first_root, 100, 101).unwrap();
    let second =
        write_run_record(&paths, "org.example.App", "second", &second_root, 200, 201).unwrap();

    assert_ne!(first, second);
    assert_eq!(read_run_records(&paths).unwrap().len(), 2);
    remove_run_record(&first).unwrap();
    let remaining = read_run_records(&paths).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        remaining[0].get("instance_id").map(String::as_str),
        Some("second")
    );
    assert_eq!(
        remaining[0].get("root").map(String::as_str),
        second_root.to_str()
    );

    let _ = fs::remove_dir_all(&temp);
}

#[test]
fn checkout_pin_records_both_pending_deployments() {
    let temp = std::env::temp_dir().join(format!(
        "freebsd-flatpak-checkout-pin-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&temp);
    let paths = Installation::for_test(&temp);
    let root = paths.chroots().join("org.example.App/install");
    let app_dir = paths.apps().join("org.example.App/commit");
    let runtime_dir = paths.runtimes().join("org.example.Platform/commit");

    let record = write_checkout_pin(
        &paths,
        "org.example.App",
        "install",
        &root,
        &app_dir,
        &runtime_dir,
    )
    .unwrap();
    let values = read_run_records(&paths).unwrap();
    let launcher_identity =
        crate::process_identity::ProcessIdentity::for_pid(std::process::id() as libc::pid_t)
            .unwrap()
            .unwrap();
    assert_eq!(
        values[0].get("launcher_start"),
        Some(&launcher_identity.to_string())
    );
    assert!(run_record_launcher_active(&values[0]).unwrap());
    assert_eq!(
        values[0].get("app_dir"),
        Some(&app_dir.display().to_string())
    );
    assert_eq!(
        values[0].get("runtime_dir"),
        Some(&runtime_dir.display().to_string())
    );

    let mut reused_pid = values[0].clone();
    reused_pid.insert("launcher_start".to_string(), "0:0".to_string());
    assert!(!run_record_launcher_active(&reused_pid).unwrap());

    let mut missing_pid = values[0].clone();
    missing_pid.insert("launcher_pid".to_string(), i32::MAX.to_string());
    assert!(!run_record_launcher_active(&missing_pid).unwrap());

    let mut legacy = values[0].clone();
    legacy.remove("launcher_start");
    assert!(run_record_launcher_active(&legacy).unwrap());

    legacy.insert("launcher_pid".to_string(), i32::MAX.to_string());
    assert!(!run_record_launcher_active(&legacy).unwrap());

    let mut malformed = values[0].clone();
    malformed.insert("launcher_start".to_string(), "invalid".to_string());
    assert!(run_record_launcher_active(&malformed).is_err());

    remove_run_record(&record).unwrap();
    let _ = fs::remove_dir_all(&temp);
}

#[test]
fn extension_pins_record_the_exact_deployment_generation() {
    let temp = std::env::temp_dir().join(format!(
        "freebsd-flatpak-extension-pin-{}",
        std::process::id()
    ));
    let paths = Installation::for_test(&temp);
    let app = AppRecord {
        origin: "flathub".into(),
        runtime_origin: "flathub".into(),
        app_id: "org.example.App".into(),
        app_ref: "app/org.example.App/x86_64/1".into(),
        app_commit: "app".into(),
        installed_size: 0,
        app_dir: "apps/app".into(),
        arch: "x86_64".into(),
        branch: "1".into(),
        runtime_ref: "org.example.Platform/x86_64/1".into(),
        runtime_commit: "runtime".into(),
        runtime_dir: "runtimes/runtime".into(),
        command: "example".into(),
    };
    let extension_dir = paths
        .runtimes()
        .join("org.example.Extension-x86_64-1/old-commit");
    write_pinned_run_record_with_extension_deployments(
        &paths,
        "instance",
        &paths.chroots().join("instance"),
        std::process::id(),
        0,
        &app,
        &["runtime/org.example.Extension/x86_64/1".to_string()],
        std::slice::from_ref(&extension_dir),
    )
    .unwrap();
    let record = &read_run_records(&paths).unwrap()[0];
    assert_eq!(
        record.get("extension_dirs"),
        Some(&extension_dir.display().to_string())
    );
    let _ = fs::remove_dir_all(temp);
}
