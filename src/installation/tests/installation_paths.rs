use super::*;

#[test]
fn test_layout_keeps_storage_classes_separate() {
    let root = Path::new("/tmp/layout-test");
    let paths = Installation::for_test(root);
    assert_eq!(
        paths.app("org.example.App"),
        root.join("xdg-data/freebsd-flatpak/apps/org.example.App")
    );
    assert_eq!(paths.repo(), root.join("xdg-data/freebsd-flatpak/repo"));
    assert_eq!(
        paths.chroots(),
        root.join("xdg-runtime/freebsd-flatpak/chroots")
    );
    assert_eq!(
        paths.app_data("org.example.App").unwrap(),
        root.join("home/.var/app/org.example.App")
    );
    assert_eq!(
        paths.portal_documents(),
        root.join("xdg-data/freebsd-flatpak/portal-documents")
    );
}
