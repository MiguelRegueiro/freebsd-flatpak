use super::*;
use crate::cli::test_support::test_dir;

fn write_runtime(paths: &Installation, runtime_ref: &str) {
    state::ensure_layout(paths).unwrap();
    state::write_runtime(
        paths,
        &state::RuntimeRecord {
            origin: "flathub".to_string(),
            runtime_ref: runtime_ref.to_string(),
            runtime_commit: format!("commit-{}", runtime_ref.replace('/', "-")),
            installed_size: 42,
            explicitly_installed: true,
            runtime_dir: PathBuf::from(format!("runtimes/{}", runtime_ref.replace('/', "-"))),
        },
    )
    .unwrap();
}

#[test]
fn runtime_options_parse_before_the_app_and_preserve_app_arguments() {
    let parsed = parse_run_args(vec![
        "--runtime=org.example.Sdk".to_string(),
        "--runtime-version".to_string(),
        "beta".to_string(),
        "org.example.App".to_string(),
        "--".to_string(),
        "--app-option".to_string(),
    ])
    .unwrap();

    assert_eq!(parsed.app_id, "org.example.App");
    assert_eq!(parsed.runtime.as_deref(), Some("org.example.Sdk"));
    assert_eq!(parsed.runtime_version.as_deref(), Some("beta"));
    assert_eq!(parsed.resolve.args, ["--app-option"]);
}

#[test]
fn runtime_override_fills_missing_parts_and_version_wins() {
    let root = test_dir("run-runtime-override");
    let paths = Installation::for_test(&root);
    write_runtime(&paths, "org.example.Sdk/x86_64/beta");

    let runtime = resolve_runtime_override(
        &paths,
        "org.example.Platform/x86_64/stable",
        Some("org.example.Sdk//old"),
        Some("beta"),
    )
    .unwrap();

    assert_eq!(runtime.runtime_ref, "org.example.Sdk/x86_64/beta");
}

#[test]
fn runtime_override_requires_an_installed_runtime_and_runtime_kind() {
    let root = test_dir("run-runtime-errors");
    let paths = Installation::for_test(&root);
    state::ensure_layout(&paths).unwrap();

    let missing = resolve_runtime_override(
        &paths,
        "org.example.Platform/x86_64/stable",
        None,
        Some("beta"),
    )
    .unwrap_err();
    assert!(missing
        .to_string()
        .contains("runtime/org.example.Platform/x86_64/beta is not installed"));
    assert!(resolve_runtime_override(
        &paths,
        "org.example.Platform/x86_64/stable",
        Some("app/org.example.Other"),
        None,
    )
    .is_err());
}
