use super::*;

#[test]
fn parses_repeated_columns_and_filters() {
    let args = ["--app", "--columns=application", "--columns", "size"].map(str::to_string);
    let options = parse_options(&args).unwrap();
    assert!(options.apps);
    assert!(!options.runtimes);
    assert_eq!(options.columns, [Column::Application, Column::Size]);
}

#[test]
fn details_selects_every_truthful_column() {
    let options = parse_options(&["--show-details".to_string()]).unwrap();
    assert_eq!(options.columns, ALL_COLUMNS);
}

#[test]
fn all_and_app_runtime_filters_parse() {
    let options =
        parse_options(&["--all", "--app-runtime=org.example.Platform//50"].map(str::to_string))
            .unwrap();
    assert!(options.all);
    assert!(options.apps);
    assert!(!options.runtimes);
    let runtime = options.app_runtime.unwrap();
    assert_eq!(runtime.id, "org.example.Platform");
    assert_eq!(runtime.arch, None);
    assert_eq!(runtime.branch.as_deref(), Some("50"));

    assert!(parse_options(
        &["--runtime", "--app-runtime=org.example.Platform"].map(str::to_string)
    )
    .is_err());
}

#[test]
fn validates_columns_and_unique_prefixes() {
    assert_eq!(resolve_column("siz").unwrap(), Some(Column::Size));
    assert!(resolve_column("missing").is_err());
    assert!(resolve_column("").is_err());
}

fn example_row() -> InstalledRow {
    InstalledRow {
        name: "Example".to_string(),
        application: "org.example.App".to_string(),
        arch: "x86_64".to_string(),
        version: "2.4".to_string(),
        branch: "stable".to_string(),
        runtime: "org.example.Platform/x86_64/1".to_string(),
        ref_name: "app/org.example.App/x86_64/stable".to_string(),
        origin: "example-origin".to_string(),
        active: "abc123".to_string(),
        installed_size: 1024,
    }
}

#[test]
fn default_table_has_new_columns_and_wider_spacing() {
    let options = parse_options(&[]).unwrap();
    assert_eq!(options.columns, DEFAULT_COLUMNS);
    assert_eq!(
        render(&[example_row()], &options, false, None),
        concat!(
            "Name       Application ID     Version    Branch    Origin\n",
            "Example    org.example.App    2.4        stable    example-origin\n",
        )
    );
}

#[test]
fn table_bolds_only_headers_when_styled() {
    let options = parse_options(&[]).unwrap();
    let plain = render(&[example_row()], &options, false, None);
    let styled = render(&[example_row()], &options, true, None);

    assert!(!plain.contains("\x1b["));
    assert!(styled.starts_with("\x1b[1mName\x1b[0m"));
    assert!(styled.contains("\x1b[1mApplication ID\x1b[0m"));
    assert!(!styled.lines().nth(1).unwrap().contains("\x1b["));
}

#[test]
fn narrow_table_truncates_cells_with_ellipses() {
    let options = parse_options(&["--columns=application,origin".to_string()]).unwrap();

    assert_eq!(
        render(&[example_row()], &options, false, Some(20)),
        concat!("Applica…    Origin\n", "org.exa…    example…\n")
    );
}

#[test]
fn narrow_all_columns_stay_on_one_line() {
    let options = parse_options(&["--columns=all".to_string()]).unwrap();
    let output = render(&[example_row()], &options, false, Some(50));

    assert!(output.contains('…'));
    for line in output.lines() {
        assert!(line.chars().count() <= 50, "line is too wide: {line:?}");
        assert_eq!(line.split("   ").count(), ALL_COLUMNS.len());
    }
}
