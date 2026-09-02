use super::*;
use crate::cli::confirmation::TransactionOptions;
use crate::cli::test_support::*;
use crate::installation as state;
use crate::installation::installation_paths::Installation;
use crate::remotes;
use std::path::PathBuf;

#[test]
fn transaction_options_parse_with_update_target() {
    let update = parse_update_args(vec![
        "--noninteractive".to_string(),
        "--no-related".to_string(),
        "org.example.App".to_string(),
    ])
    .unwrap();
    assert!(update.transaction.noninteractive);
    assert!(update.no_related);
}

#[test]
fn newer_remote_app_commit_requires_app_checkout() {
    let root = test_dir("newer-app-commit");
    let paths = Installation::for_test(&root);
    create_checkout(
        paths.data_root(),
        &PathBuf::from("apps").join("org.example.App"),
    );
    create_runtime_checkout(&paths);
    let mut record = app_record(
        "org.example.App",
        "app/org.example.App/x86_64/stable",
        "app-1",
    );
    record.command = "new-command".to_string();
    let remote = remote_app(
        "org.example.App",
        "app/org.example.App/x86_64/stable",
        "app-2",
    );

    let status = update_status(&paths, &record, &remote).unwrap();

    assert!(status.app_changed);
    assert!(status.app_checkout_stale);
    assert!(!status.runtime_changed);
    assert!(!status.runtime_checkout_stale);
}

#[test]
fn active_run_does_not_block_noop_status_or_unrelated_target_selection() {
    let root = test_dir("active-noop-unrelated");
    let paths = Installation::for_test(&root);
    create_checkout(
        paths.data_root(),
        &PathBuf::from("apps").join("org.example.App"),
    );
    create_runtime_checkout(&paths);
    let mut record = app_record(
        "org.example.App",
        "app/org.example.App/x86_64/stable",
        "app-1",
    );
    record.command = "new-command".to_string();
    state::write_run_record(
        &paths,
        &record.app_id,
        "active",
        &paths.chroots().join("active"),
        std::process::id(),
        0,
    )
    .unwrap();
    let remote = remote_app(&record.app_id, &record.app_ref, &record.app_commit);
    let status = update_status(&paths, &record, &remote).unwrap();
    assert!(!status.app_changed);
    assert!(!status.runtime_changed);

    let other = app_record(
        "org.example.Other",
        "app/org.example.Other/x86_64/stable",
        "other-1",
    );
    let metadata = remotes::RemoteMetadata::empty_for_test(&root);
    let selected = update_targets(
        vec![record, other.clone()],
        vec![other.app_id.clone()],
        &metadata,
    )
    .unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].record.app_id, other.app_id);
}

#[test]
fn app_id_replacement_requires_app_checkout_even_with_same_commit() {
    let root = test_dir("replacement");
    let paths = Installation::for_test(&root);
    create_checkout(
        paths.data_root(),
        &PathBuf::from("apps").join("org.example.OldApp"),
    );
    create_runtime_checkout(&paths);
    let mut record = app_record(
        "org.example.OldApp",
        "app/org.example.OldApp/x86_64/stable",
        "app-1",
    );
    record.command = "new-command".to_string();
    let remote = remote_app(
        "org.example.NewApp",
        "app/org.example.NewApp/x86_64/stable",
        "app-1",
    );

    let status = update_status(&paths, &record, &remote).unwrap();

    assert!(status.app_changed);
    assert!(status.app_checkout_stale);
}

#[test]
fn missing_runtime_checkout_requires_runtime_checkout() {
    let root = test_dir("missing-runtime");
    let paths = Installation::for_test(&root);
    create_checkout(
        paths.data_root(),
        &PathBuf::from("apps").join("org.example.App"),
    );
    let mut record = app_record(
        "org.example.App",
        "app/org.example.App/x86_64/stable",
        "app-1",
    );
    record.command = "new-command".to_string();
    let remote = remote_app(
        "org.example.App",
        "app/org.example.App/x86_64/stable",
        "app-1",
    );

    let status = update_status(&paths, &record, &remote).unwrap();

    assert!(status.runtime_changed);
    assert!(status.runtime_checkout_stale);
    assert_eq!(status.current_runtime_commit.as_deref(), Some("runtime-1"));
}

#[test]
fn runtime_branch_change_reports_the_apps_previous_runtime_commit() {
    let root = test_dir("runtime-branch-reporting");
    let paths = Installation::for_test(&root);
    state::ensure_layout(&paths).unwrap();
    create_checkout(
        paths.data_root(),
        &PathBuf::from("apps").join("org.example.App"),
    );
    let runtime_50_dir = paths.runtimes().join("platform-50");
    create_checkout(paths.data_root(), &PathBuf::from("runtimes/platform-50"));
    state::write_runtime(
        &paths,
        &state::RuntimeRecord {
            origin: "flathub".to_string(),
            runtime_ref: "org.example.Platform/x86_64/50".to_string(),
            runtime_commit: "runtime-50".to_string(),
            explicitly_installed: false,
            installed_size: 0,
            runtime_dir: paths.relative_data_path(&runtime_50_dir).unwrap(),
        },
    )
    .unwrap();

    let mut record = app_record(
        "org.example.App",
        "app/org.example.App/x86_64/stable",
        "app-1",
    );
    record.command = "new-command".to_string();
    record.runtime_ref = "org.example.Platform/x86_64/49".to_string();
    record.runtime_commit = "runtime-49".to_string();
    let mut remote = remote_app(&record.app_id, &record.app_ref, &record.app_commit);
    remote.runtime_ref = "org.example.Platform/x86_64/50".to_string();
    remote.runtime_commit = "runtime-50".to_string();

    let status = update_status(&paths, &record, &remote).unwrap();

    assert!(status.runtime_changed);
    assert!(!status.runtime_checkout_stale);
    assert_eq!(status.current_runtime_commit.as_deref(), Some("runtime-49"));
}

#[test]
fn stale_record_command_updates_state_without_app_checkout() {
    let root = test_dir("state-only-command");
    let paths = Installation::for_test(&root);
    create_checkout(
        paths.data_root(),
        &PathBuf::from("apps").join("org.example.App"),
    );
    create_runtime_checkout(&paths);
    let record = app_record(
        "org.example.App",
        "app/org.example.App/x86_64/stable",
        "app-1",
    );
    let remote = remote_app(
        "org.example.App",
        "app/org.example.App/x86_64/stable",
        "app-1",
    );

    let status = update_status(&paths, &record, &remote).unwrap();

    assert!(status.app_changed);
    assert!(!status.app_checkout_stale);
}

#[test]
fn older_remote_app_commit_requires_app_checkout_for_downgrade() {
    let root = test_dir("older-app-commit");
    let paths = Installation::for_test(&root);
    create_checkout(
        paths.data_root(),
        &PathBuf::from("apps").join("org.example.App"),
    );
    create_runtime_checkout(&paths);
    let mut record = app_record(
        "org.example.App",
        "app/org.example.App/x86_64/stable",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    );
    record.command = "new-command".to_string();
    let remote = remote_app(
        "org.example.App",
        "app/org.example.App/x86_64/stable",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );

    let status = update_status(&paths, &record, &remote).unwrap();

    assert!(status.app_changed);
    assert!(status.app_checkout_stale);
}

#[test]
fn update_commit_requires_exactly_one_app() {
    assert_eq!(
        parse_update_args(vec![
            "--commit=abc123".to_string(),
            "org.example.App".to_string()
        ])
        .unwrap(),
        UpdateOptions {
            transaction: TransactionOptions::default(),
            commit: Some("abc123".to_string()),
            no_related: false,
            kind: None,
            refs: vec!["org.example.App".to_string()],
        }
    );
    assert!(parse_update_args(vec!["--commit=abc123".to_string()]).is_err());
    assert!(parse_update_args(vec![
        "--commit=abc123".to_string(),
        "org.example.One".to_string(),
        "org.example.Two".to_string(),
    ])
    .is_err());
}

#[test]
fn runtime_update_filter_parses() {
    let update = parse_update_args(vec![
        "--runtime".to_string(),
        "runtime/org.example.Platform/x86_64/50".to_string(),
    ])
    .unwrap();
    assert_eq!(update.kind, Some(RefKind::Runtime));
    assert!(parse_update_args(vec![
        "--app".to_string(),
        "--runtime".to_string(),
        "org.example.Ref".to_string(),
    ])
    .is_err());
}

#[test]
fn targeted_runtime_update_never_selects_all_apps_after_consuming_its_operand() {
    let root = test_dir("targeted-runtime-only");
    let paths = Installation::for_test(&root);
    state::ensure_layout(&paths).unwrap();
    let app = app_record(
        "org.example.App",
        "app/org.example.App/x86_64/stable",
        "app-commit",
    );
    state::write_app(&paths, &app).unwrap();
    state::write_runtime(
        &paths,
        &state::RuntimeRecord {
            origin: "runtime-origin".to_string(),
            runtime_ref: app.runtime_ref.clone(),
            runtime_commit: app.runtime_commit.clone(),
            installed_size: 42,
            explicitly_installed: true,
            runtime_dir: app.runtime_dir.clone(),
        },
    )
    .unwrap();
    let mut options = parse_update_args(vec![format!("runtime/{}", app.runtime_ref)]).unwrap();
    let had_requested_refs = !options.refs.is_empty();
    let (runtime_targets, matched) =
        select_runtime_targets(&paths, &options, std::slice::from_ref(&app)).unwrap();
    assert_eq!(runtime_targets.len(), 1);
    options
        .refs
        .retain(|requested| !matched.contains(requested));
    assert!(options.refs.is_empty());
    assert!(!should_update_apps(&options, had_requested_refs));
}
