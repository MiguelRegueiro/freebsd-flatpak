use super::*;
use std::fs;

#[test]
fn parses_one_or_more_extra_data_sources() {
    let sources = parse_sources(
        "[Extra Data]\nname=first.bin\nuri=https://example.com/first\nsize=3\nchecksum=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nname1=second.bin\nuri1=http://example.com/second\nsize1=4\nchecksum1=BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB\n",
    )
    .unwrap();
    assert_eq!(sources.len(), 2);
    assert_eq!(sources[0].name, "first.bin");
    assert_eq!(sources[1].name, "second.bin");
    assert_eq!(
        sources[1].checksum,
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    );
}

#[test]
fn rejects_unsafe_extra_data_names_and_uris() {
    let metadata = "[Extra Data]\nname=../payload\nuri=https://example.com/file\nsize=1\nchecksum=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n";
    assert!(parse_sources(metadata)
        .unwrap_err()
        .to_string()
        .contains("invalid extra-data filename"));

    let metadata = "[Extra Data]\nname=payload\nuri=file:///tmp/file\nsize=1\nchecksum=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n";
    assert!(parse_sources(metadata)
        .unwrap_err()
        .to_string()
        .contains("unsupported extra-data URI"));
}

#[test]
fn verifies_size_and_sha256() {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-extra-data-test-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let path = root.join("payload");
    fs::write(&path, b"abc").unwrap();
    let source = ExtraDataSource {
        name: "payload".to_string(),
        uri: "https://example.com/payload".to_string(),
        size: 3,
        checksum: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_string(),
    };
    verify_download(&path, &source).unwrap();

    let mut wrong = source.clone();
    wrong.size = 2;
    assert!(verify_download(&path, &wrong)
        .unwrap_err()
        .to_string()
        .contains("expected 2"));
    let _ = fs::remove_dir_all(root);
}
