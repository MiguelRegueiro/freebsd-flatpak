use super::*;
use std::fs;

fn test_paths(name: &str) -> Installation {
    let root =
        std::env::temp_dir().join(format!("freebsd-flatpak-ps-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    Installation::for_test(&root)
}

fn write_app(paths: &Installation) {
    state::ensure_layout(paths).unwrap();
    fs::create_dir_all(paths.refs().join("apps")).unwrap();
    fs::write(
        paths.refs().join("apps/app.zen_browser.zen.ini"),
        "app_id=app.zen_browser.zen\napp_ref=app/app.zen_browser.zen/x86_64/stable\napp_commit=app-commit\ninstalled_size=1234\napp_dir=apps/app.zen_browser.zen\narch=x86_64\nbranch=stable\nruntime_ref=org.freedesktop.Platform/x86_64/25.08\nruntime_commit=runtime-commit\nruntime_dir=runtimes/org.freedesktop.Platform-25.08\ncommand=zen\n",
    )
    .unwrap();
}

#[test]
fn default_and_selected_columns_match_flatpak_ps() {
    let paths = test_paths("columns");
    write_app(&paths);
    state::write_run_record(
        &paths,
        "app.zen_browser.zen",
        "815848674",
        &paths.chroots().join("zen/815848674"),
        std::process::id(),
        5960,
    )
    .unwrap();

    let default = output(&paths, Vec::new()).unwrap();
    assert!(default.starts_with("Instance  PID"));
    assert!(default.contains("815848674"));
    assert!(default.contains("app.zen_browser.zen"));
    assert!(default.contains("org.freedesktop.Platform"));

    let selected = output(
        &paths,
        vec!["--columns=instance,application,pid,child-pid".to_string()],
    )
    .unwrap();
    assert!(selected.starts_with("Instance  Application"));
    assert!(selected.contains(&format!("{} 5960", std::process::id())));
}

#[test]
fn stale_records_are_not_shown() {
    let paths = test_paths("stale");
    write_app(&paths);
    state::write_run_record(
        &paths,
        "app.zen_browser.zen",
        "stale",
        &paths.chroots().join("zen/stale"),
        i32::MAX as u32,
        0,
    )
    .unwrap();

    assert_eq!(output(&paths, Vec::new()).unwrap(), "");
}

#[test]
fn pinned_columns_do_not_follow_a_later_current_generation() {
    let paths = test_paths("pinned-generation");
    write_app(&paths);
    let old = state::get_app(&paths, "app.zen_browser.zen").unwrap();
    state::write_pinned_run_record(
        &paths,
        "pinned",
        &paths.chroots().join("pinned"),
        std::process::id(),
        0,
        &old,
    )
    .unwrap();
    fs::write(
        paths.refs().join("apps/app.zen_browser.zen.ini"),
        "app_id=app.zen_browser.zen\napp_ref=app/app.zen_browser.zen/x86_64/stable\napp_commit=app-new\ninstalled_size=5678\napp_dir=apps/app.zen_browser.zen/app-new\narch=x86_64\nbranch=stable\nruntime_ref=org.freedesktop.Platform/x86_64/25.08\nruntime_commit=runtime-new\nruntime_dir=runtimes/org.freedesktop.Platform-25.08/runtime-new\ncommand=zen\n",
    )
    .unwrap();

    let output = output(&paths, vec!["--columns=commit,runtime-commit".to_string()]).unwrap();
    assert!(output.contains("app-commit"));
    assert!(output.contains("runtime-commit"));
    assert!(!output.contains("app-new"));
    assert!(!output.contains("runtime-new"));
}

#[test]
fn active_legacy_records_use_the_launcher_pid_as_instance() {
    let paths = test_paths("legacy");
    write_app(&paths);
    fs::write(
        paths.runs().join("app.zen_browser.zen.ini"),
        format!(
            "app_id=app.zen_browser.zen\nroot=/legacy\nlauncher_pid={}\nchild_pid=5960\n",
            std::process::id()
        ),
    )
    .unwrap();

    let result = output(
        &paths,
        vec!["--columns=instance,application,pid,child-pid".to_string()],
    )
    .unwrap();
    let pid = std::process::id().to_string();
    assert!(result.lines().nth(1).unwrap().starts_with(&pid));
    assert!(result.contains(&format!("{pid} 5960")));
}

#[test]
fn columns_accept_unique_prefixes_and_reject_unknown_names() {
    assert_eq!(
        parse_columns(vec!["--columns=inst,app,child,runtime".to_string()]).unwrap(),
        vec![
            Column::Instance,
            Column::Application,
            Column::ChildPid,
            Column::Runtime
        ]
    );
    assert!(parse_columns(vec!["--columns=nope".to_string()]).is_err());
}
