use super::*;
use std::fs;

#[test]
fn historical_commit_uses_an_immutable_generation_path() {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-runtime-generation-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let base = root.join("org.example.App");
    fs::create_dir_all(base.join("commit-b")).unwrap();

    assert_eq!(
        generation_checkout_dir(&base, "commit-a", false),
        base.join("commit-a")
    );
    assert_eq!(
        generation_checkout_dir(&base, "commit-b", false),
        base.join("commit-b")
    );
    let repaired = generation_checkout_dir(&base, "commit-b", true);
    assert_ne!(repaired, base.join("commit-b"));
    assert!(repaired.starts_with(&base));
    let _ = fs::remove_dir_all(&root);
}
