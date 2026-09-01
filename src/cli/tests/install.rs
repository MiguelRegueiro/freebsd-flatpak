use super::*;

#[test]
fn transaction_and_or_update_options_parse_together() {
    let install = parse_install_args(vec![
        "--or-update".to_string(),
        "-y".to_string(),
        "org.example.App".to_string(),
    ])
    .unwrap();
    assert!(install.or_update);
    assert!(install.transaction.assumeyes);
}

#[test]
fn named_remote_and_full_ref_parse() {
    let install = parse_install_args(vec![
        "flathub".to_string(),
        "app/org.example.App/x86_64/stable".to_string(),
    ])
    .unwrap();
    assert_eq!(install.remote.as_deref(), Some("flathub"));
    assert_eq!(install.ref_name, "app/org.example.App/x86_64/stable");
}

#[test]
fn kind_filters_parse_and_conflict() {
    let runtime = parse_install_args(vec![
        "--runtime".to_string(),
        "org.example.Platform/x86_64/50".to_string(),
    ])
    .unwrap();
    assert_eq!(runtime.kind, Some(RefKind::Runtime));
    assert!(parse_install_args(vec![
        "--app".to_string(),
        "--runtime".to_string(),
        "org.example.Ref".to_string(),
    ])
    .is_err());
}
