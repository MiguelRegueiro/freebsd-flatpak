use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

#[test]
fn installed_appstream_fields_use_matching_component() {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-list-metadata-{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    let metainfo = root.join("files/share/metainfo");
    fs::create_dir_all(&metainfo).unwrap();
    fs::write(
        metainfo.join("org.example.App.metainfo.xml"),
        r#"<component><id>org.example.App.desktop</id><name>Example App</name><releases><release version="2.4"/></releases></component>"#,
    )
    .unwrap();

    assert_eq!(
        installed_appstream_fields(&root, "org.example.App"),
        ("Example App".to_string(), "2.4".to_string())
    );
    assert_eq!(
        installed_appstream_fields(&root, "org.example.Missing"),
        (String::new(), String::new())
    );

    fs::remove_dir_all(root).unwrap();
}
