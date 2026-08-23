use super::*;
use crate::commands::test_support::*;
use crate::{paths::Installation, runtime, state};
use std::collections::BTreeSet;
use std::fs;

#[test]
fn transaction_and_delete_data_options_parse_together() {
    let uninstall = parse_uninstall_args(vec![
        "--delete-data".to_string(),
        "--assumeyes".to_string(),
        "org.example.App".to_string(),
    ])
    .unwrap();
    assert!(uninstall.delete_data);
    assert!(uninstall.transaction.assumeyes);
    assert!(
        parse_uninstall_args(vec!["--unused".to_string(), "--delete-data".to_string(),]).is_err()
    );
}

#[test]
fn delete_data_removes_only_the_requested_apps_persistent_directory() {
    let root = test_dir("delete-data");
    let paths = Installation::for_test(&root);
    state::ensure_layout(&paths).unwrap();
    let requested = paths.app_data("org.example.App").unwrap();
    let other = paths.app_data("org.example.Other").unwrap();
    fs::create_dir_all(&requested).unwrap();
    fs::create_dir_all(&other).unwrap();
    fs::write(requested.join("settings"), "app").unwrap();
    fs::write(other.join("settings"), "other").unwrap();

    remove_app_data(&paths, "org.example.App").unwrap();

    assert!(!requested.exists());
    assert_eq!(fs::read_to_string(other.join("settings")).unwrap(), "other");
}

#[test]
fn uninstall_unused_preserves_installed_and_pinned_dependencies() {
    let root = test_dir("unused");
    let paths = Installation::for_test(&root);
    state::ensure_layout(&paths).unwrap();
    let runtime_one = "org.example.Platform/x86_64/one";
    let runtime_two = "org.example.Platform/x86_64/two";
    let runtime_three = "org.example.Platform/x86_64/three";
    let app_dir = paths.app("org.example.App").join("app-current");
    let runtime_one_dir = paths.runtimes().join("platform-one").join("runtime-one");
    let app_metadata = format!(
            "[Application]\nname=org.example.App\nruntime={runtime_one}\ncommand=example\n\n[Extension org.example.Keep]\ndirectory=lib/keep\nversion=one\n"
        );
    create_marked_checkout(
        &app_dir,
        "app/org.example.App/x86_64/stable",
        "app-current",
        &app_metadata,
    );
    create_marked_checkout(
            &runtime_one_dir,
            &format!("runtime/{runtime_one}"),
            "runtime-one",
            "[Runtime]\nname=org.example.Platform\n\n[Extension org.freedesktop.Platform.GL]\ndirectory=lib/x86_64-linux-gnu/GL\nversions=one;one-extra;1.4\nsubdirectories=true\ndownload-if=active-gl-driver\nenable-if=active-gl-driver\nautoprune-unless=active-gl-driver\n",
        );
    state::record_install(
        &paths,
        &runtime::InstalledApp {
            app_id: "org.example.App".to_string(),
            app_ref: "app/org.example.App/x86_64/stable".to_string(),
            app_commit: "app-current".to_string(),
            app_dir: app_dir.clone(),
            arch: "x86_64".to_string(),
            branch: "stable".to_string(),
            runtime_ref: runtime_one.to_string(),
            runtime_commit: "runtime-one".to_string(),
            runtime_dir: runtime_one_dir.clone(),
            command: "example".to_string(),
            timings: Default::default(),
        },
    )
    .unwrap();

    let pinned_app_dir = paths.app("org.example.Old").join("app-old");
    let runtime_two_dir = paths.runtimes().join("platform-two").join("runtime-two");
    create_marked_checkout(
            &pinned_app_dir,
            "app/org.example.Old/x86_64/stable",
            "app-old",
            &format!(
                "[Application]\nname=org.example.Old\nruntime={runtime_two}\ncommand=old\n\n[Extension org.example.Active]\ndirectory=lib/active\nversion=two\n"
            ),
        );
    create_marked_checkout(
        &runtime_two_dir,
        &format!("runtime/{runtime_two}"),
        "runtime-two",
        "[Runtime]\nname=org.example.Platform\n",
    );
    let pinned = state::AppRecord {
        app_id: "org.example.Old".to_string(),
        app_ref: "app/org.example.Old/x86_64/stable".to_string(),
        app_commit: "app-old".to_string(),
        app_dir: paths.relative_data_path(&pinned_app_dir).unwrap(),
        arch: "x86_64".to_string(),
        branch: "stable".to_string(),
        runtime_ref: runtime_two.to_string(),
        runtime_commit: "runtime-two".to_string(),
        runtime_dir: paths.relative_data_path(&runtime_two_dir).unwrap(),
        command: "old".to_string(),
    };
    state::write_runtime(
        &paths,
        &state::RuntimeRecord {
            runtime_ref: runtime_two.to_string(),
            runtime_commit: "runtime-two".to_string(),
            runtime_dir: pinned.runtime_dir.clone(),
        },
    )
    .unwrap();
    state::write_pinned_run_record_with_extensions(
        &paths,
        "active-old",
        &paths.chroots().join("active-old"),
        std::process::id(),
        0,
        &pinned,
        &["runtime/org.example.PinnedOnly/x86_64/two".to_string()],
    )
    .unwrap();

    let runtime_three_dir = paths
        .runtimes()
        .join("platform-three")
        .join("runtime-three");
    create_marked_checkout(
        &runtime_three_dir,
        &format!("runtime/{runtime_three}"),
        "runtime-three",
        "[Runtime]\nname=org.example.Platform\n",
    );
    state::write_runtime(
        &paths,
        &state::RuntimeRecord {
            runtime_ref: runtime_three.to_string(),
            runtime_commit: "runtime-three".to_string(),
            runtime_dir: paths.relative_data_path(&runtime_three_dir).unwrap(),
        },
    )
    .unwrap();

    for (name, ref_name) in [
        ("keep", "runtime/org.example.Keep/x86_64/one"),
        ("active", "runtime/org.example.Active/x86_64/two"),
        (
            "gl-default",
            "runtime/org.freedesktop.Platform.GL.default/x86_64/one",
        ),
        ("pinned-only", "runtime/org.example.PinnedOnly/x86_64/two"),
        ("unused", "runtime/org.example.Unused/x86_64/one"),
    ] {
        create_marked_checkout(
            &paths.extensions().join(name),
            ref_name,
            name,
            "[Runtime]\nname=extension\n",
        );
    }

    let plan = plan_unused_deployment_checkouts(&paths).unwrap();
    let planned_refs = plan
        .iter()
        .map(|item| item.ref_name.clone())
        .collect::<BTreeSet<_>>();
    assert!(planned_refs.contains(&format!("runtime/{runtime_three}")));
    assert!(planned_refs.contains("runtime/org.example.Unused/x86_64/one"));
    assert!(runtime_three_dir.exists());
    assert!(paths.extensions().join("unused").exists());

    let removed = apply_unused_deployment_plan(&paths, plan).unwrap();
    assert!(removed.contains(&format!("runtime/{runtime_three}")));
    assert!(removed.contains(&"runtime/org.example.Unused/x86_64/one".to_string()));
    assert!(runtime_one_dir.exists());
    assert!(runtime_two_dir.exists());
    assert!(!runtime_three_dir.exists());
    assert!(paths.extensions().join("keep").exists());
    assert!(paths.extensions().join("active").exists());
    assert!(paths.extensions().join("gl-default").exists());
    assert!(paths.extensions().join("pinned-only").exists());
    assert!(!paths.extensions().join("unused").exists());
}

#[test]
fn normal_app_uninstall_leaves_runtime_for_unused_cleanup() {
    let root = test_dir("uninstall-then-unused");
    let paths = Installation::for_test(&root);
    state::ensure_layout(&paths).unwrap();
    let app_id = "org.gnome.Calculator";
    let runtime_ref = "org.gnome.Platform/x86_64/50";
    let app_dir = paths.app(app_id).join("calculator-commit");
    let runtime_dir = paths
        .runtimes()
        .join("org.gnome.Platform-50")
        .join("runtime-commit");
    create_marked_checkout(
        &app_dir,
        "app/org.gnome.Calculator/x86_64/stable",
        "calculator-commit",
        &format!("[Application]\nname={app_id}\nruntime={runtime_ref}\ncommand=gnome-calculator\n"),
    );
    create_marked_checkout(
        &runtime_dir,
        &format!("runtime/{runtime_ref}"),
        "runtime-commit",
        "[Runtime]\nname=org.gnome.Platform\n",
    );
    state::record_install(
        &paths,
        &runtime::InstalledApp {
            app_id: app_id.to_string(),
            app_ref: "app/org.gnome.Calculator/x86_64/stable".to_string(),
            app_commit: "calculator-commit".to_string(),
            app_dir: app_dir.clone(),
            arch: "x86_64".to_string(),
            branch: "stable".to_string(),
            runtime_ref: runtime_ref.to_string(),
            runtime_commit: "runtime-commit".to_string(),
            runtime_dir: runtime_dir.clone(),
            command: "gnome-calculator".to_string(),
            timings: Default::default(),
        },
    )
    .unwrap();

    // This is the deployment-state transition performed by ordinary app
    // uninstall before repository refs and user-facing output are handled.
    let removed_app = state::remove_app_record(&paths, app_id).unwrap().unwrap();
    state::safe_remove_dir(&paths, &removed_app.app_dir).unwrap();
    state::cleanup_retired_deployments(&paths).unwrap();
    assert!(state::list_apps(&paths).unwrap().is_empty());
    assert!(state::get_runtime(&paths, runtime_ref).unwrap().is_some());
    assert!(runtime_dir.exists());

    let removed = remove_unused_deployment_checkouts(&paths).unwrap();
    assert_eq!(removed, vec![format!("runtime/{runtime_ref}")]);
    assert!(state::get_runtime(&paths, runtime_ref).unwrap().is_none());
    assert!(!runtime_dir.exists());
}

#[test]
fn unused_cleanup_discovers_orphan_runtime_without_inventory_record() {
    let root = test_dir("unused-discovered-runtime");
    let paths = Installation::for_test(&root);
    state::ensure_layout(&paths).unwrap();
    let runtime_ref = "org.gnome.Platform/x86_64/50";
    let runtime_dir = paths
        .runtimes()
        .join("org.gnome.Platform-50")
        .join("runtime-commit");
    create_marked_checkout(
        &runtime_dir,
        &format!("runtime/{runtime_ref}"),
        "runtime-commit",
        "[Runtime]\nname=org.gnome.Platform\n",
    );
    assert!(state::list_runtimes(&paths).unwrap().is_empty());

    let removed = remove_unused_deployment_checkouts(&paths).unwrap();
    assert_eq!(removed, vec![format!("runtime/{runtime_ref}")]);
    assert!(!runtime_dir.exists());
}
