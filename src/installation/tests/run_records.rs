use super::*;
use crate::installation::installation_paths::Installation;
use crate::installation::{remove_run_record, write_run_record};
use std::fs;

#[test]
fn concurrent_run_records_are_distinct_and_cleanup_is_isolated() {
    let temp = std::env::temp_dir().join(format!(
        "freebsd-flatpak-state-concurrent-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&temp);
    let paths = Installation::for_test(&temp);
    let first_root = paths.chroots().join("org.example.App/first");
    let second_root = paths.chroots().join("org.example.App/second");

    let first =
        write_run_record(&paths, "org.example.App", "first", &first_root, 100, 101).unwrap();
    let second =
        write_run_record(&paths, "org.example.App", "second", &second_root, 200, 201).unwrap();

    assert_ne!(first, second);
    assert_eq!(read_run_records(&paths).unwrap().len(), 2);
    remove_run_record(&first).unwrap();
    let remaining = read_run_records(&paths).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        remaining[0].get("instance_id").map(String::as_str),
        Some("second")
    );
    assert_eq!(
        remaining[0].get("root").map(String::as_str),
        second_root.to_str()
    );

    let _ = fs::remove_dir_all(&temp);
}
