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
    assert_eq!(install.app_id, "app/org.example.App/x86_64/stable");
}
