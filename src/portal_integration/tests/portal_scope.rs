use super::*;

fn test_paths(name: &str) -> Installation {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-portal-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    Installation::for_test(&root)
}

#[test]
fn same_app_instances_share_one_dbus_scope() {
    let paths = test_paths("shared-dbus-scope");

    let first = shared_portal_dir(&paths, "org.example.App").join("bus/bus");
    let second = shared_portal_dir(&paths, "org.example.App").join("bus/bus");
    let other = shared_portal_dir(&paths, "org.example.Other").join("bus/bus");

    assert_eq!(first, second);
    assert_ne!(first, other);
}

#[test]
fn shared_portal_survives_either_non_final_instance_and_stops_after_last() {
    let paths = test_paths("shared-lifetime");
    let first_root = paths.chroots().join("org.example.App/first");
    let second_root = paths.chroots().join("org.example.App/second");
    let other_root = paths.chroots().join("org.example.Other/only");
    let first_record = crate::installation::write_run_record(
        &paths,
        "org.example.App",
        "first",
        &first_root,
        std::process::id(),
        0,
    )
    .unwrap();
    crate::installation::write_run_record(
        &paths,
        "org.example.Other",
        "only",
        &other_root,
        std::process::id(),
        0,
    )
    .unwrap();
    let second_record = crate::installation::write_run_record(
        &paths,
        "org.example.App",
        "second",
        &second_root,
        std::process::id(),
        0,
    )
    .unwrap();

    assert!(other_active_app_instances(&paths, "org.example.App", "first").unwrap());
    assert!(other_active_app_instances(&paths, "org.example.App", "second").unwrap());

    crate::installation::remove_run_record(&first_record).unwrap();
    assert!(!other_active_app_instances(&paths, "org.example.App", "second").unwrap());
    crate::installation::write_run_record(
        &paths,
        "org.example.App",
        "first",
        &first_root,
        std::process::id(),
        0,
    )
    .unwrap();
    crate::installation::remove_run_record(&second_record).unwrap();
    assert!(!other_active_app_instances(&paths, "org.example.App", "first").unwrap());
}
