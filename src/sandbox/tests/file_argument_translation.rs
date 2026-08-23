use crate::sandbox::filesystem_grants::{HostFilesystem, HostPathGrant};
use crate::sandbox::filesystem_permissions::AccessMode;
use std::path::PathBuf;

fn test_filesystem() -> HostFilesystem {
    HostFilesystem::new_for_tests(vec![HostPathGrant::new(
        "xdg-documents",
        "xdg-documents",
        PathBuf::from("/host/home/user/Documents"),
        PathBuf::from("/home/user/Documents"),
        AccessMode::ReadWrite,
    )
    .unwrap()])
}

#[test]
fn translates_allowed_absolute_path() {
    let fs = test_filesystem();
    let args = fs
        .translate_args(&["/host/home/user/Documents/test.txt".to_string()])
        .unwrap();
    assert_eq!(args, ["/home/user/Documents/test.txt"]);
}

#[test]
fn translates_allowed_file_uri() {
    let fs = test_filesystem();
    let args = fs
        .translate_args(&["file:///host/home/user/Documents/a%20b.txt".to_string()])
        .unwrap();
    assert_eq!(args, ["file:///home/user/Documents/a%20b.txt"]);
}

#[test]
fn preserves_sandbox_internal_absolute_path() {
    let fs = test_filesystem();
    let args = fs
        .translate_args(&["/var/data/audio-test-tone.wav".to_string()])
        .unwrap();
    assert_eq!(args, ["/var/data/audio-test-tone.wav"]);
}

#[test]
fn preserves_sandbox_internal_file_uri() {
    let fs = test_filesystem();
    let args = fs
        .translate_args(&["file:///var/data/audio-test-tone.wav".to_string()])
        .unwrap();
    assert_eq!(args, ["file:///var/data/audio-test-tone.wav"]);
}

#[test]
fn drops_literal_desktop_field_codes() {
    let fs = test_filesystem();
    let args = fs
        .translate_args(&["--new-window".to_string(), "%U".to_string()])
        .unwrap();
    assert_eq!(args, ["--new-window"]);
}
