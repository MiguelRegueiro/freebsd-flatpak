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
fn validates_columns_and_unique_prefixes() {
    assert_eq!(resolve_column("siz").unwrap(), Some(Column::Size));
    assert!(resolve_column("missing").is_err());
    assert!(resolve_column("").is_err());
}
