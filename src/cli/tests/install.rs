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
