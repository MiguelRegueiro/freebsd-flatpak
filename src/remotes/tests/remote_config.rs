use super::*;

#[test]
fn parses_flatpakrepo_fields() {
    let values = parse_flatpakrepo(
        "[Flatpak Repo]\nTitle=Example\nUrl=https://example.test/repo\nGPGKey=YWJj\n",
    )
    .unwrap();
    assert_eq!(values.get("Title").map(String::as_str), Some("Example"));
    assert_eq!(
        values.get("Url").map(String::as_str),
        Some("https://example.test/repo")
    );
}

#[test]
fn remote_names_are_safe_path_components() {
    assert!(validate_name("example.org-beta").is_ok());
    for invalid in ["", ".", "..", "with/slash", "with space"] {
        assert!(validate_name(invalid).is_err(), "accepted {invalid:?}");
    }
}

#[test]
fn flathub_is_bootstrapped_once_and_deletion_is_persistent() {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-remote-bootstrap-{}-{}",
        std::process::id(),
        crate::remotes::unique_sequence()
    ));
    let paths = Installation::for_test(&root);
    paths.ensure().unwrap();
    initialize_detailed(&paths, &Diagnostics::new(Default::default())).unwrap();
    assert_eq!(list(&paths).unwrap()[0].name, DEFAULT_REMOTE);

    delete(&paths, DEFAULT_REMOTE).unwrap();
    initialize_detailed(&paths, &Diagnostics::new(Default::default())).unwrap();
    assert!(list(&paths).unwrap().is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn configurations_round_trip_independently() {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-remote-roundtrip-{}-{}",
        std::process::id(),
        crate::remotes::unique_sequence()
    ));
    let paths = Installation::for_test(&root);
    let first = Remote {
        name: "first".to_string(),
        url: "https://first.example/repo".to_string(),
        title: Some("First".to_string()),
        enabled: true,
        gpg_verify: false,
        gpg_key: None,
    };
    let mut second = first.clone();
    second.name = "second".to_string();
    second.url = "https://second.example/repo".to_string();
    second.enabled = false;
    write(&paths, &first).unwrap();
    write(&paths, &second).unwrap();
    assert_eq!(list(&paths).unwrap(), vec![first.clone(), second]);
    assert_eq!(enabled(&paths).unwrap(), vec![first]);
    let _ = std::fs::remove_dir_all(root);
}
