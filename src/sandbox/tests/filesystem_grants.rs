use super::{AccessMode, HostFilesystem};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static TEST_ID: AtomicUsize = AtomicUsize::new(0);

struct TestTree {
    root: PathBuf,
}

impl TestTree {
    fn new(name: &str) -> Self {
        let id = TEST_ID.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "freebsd-flatpak-poc-{name}-{}-{id}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }
}

impl Drop for TestTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn metadata(filesystems: &str) -> String {
    format!(
        "\
[Application]
name=org.example.App

[Context]
filesystems={filesystems}
"
    )
}

#[test]
fn metadata_without_filesystems_has_no_grants() {
    let tree = TestTree::new("no-filesystems");
    let home = tree.path("home/user");
    fs::create_dir_all(&home).unwrap();
    let fs = HostFilesystem::from_metadata(
        "[Application]\nname=org.example.App\n",
        "user",
        &home,
        &tree.path("project"),
        &tree.path("project/runtime/chroots/org.example.App"),
    )
    .unwrap();
    assert!(fs.permissions().is_empty());
    assert!(fs.grants().is_empty());
}

#[test]
fn xdg_documents_ro_resolves_read_only() {
    let tree = TestTree::new("xdg-documents-ro");
    let home = tree.path("home/user");
    let documents = home.join("Documents");
    fs::create_dir_all(&documents).unwrap();
    let fs = HostFilesystem::from_metadata(
        &metadata("xdg-documents:ro;"),
        "user",
        &home,
        &tree.path("project"),
        &tree.path("project/runtime/chroots/org.example.App"),
    )
    .unwrap();
    assert_eq!(fs.permissions().len(), 1);
    assert_eq!(fs.grants().len(), 1);
    assert_eq!(fs.grants()[0].host_path(), documents.as_path());
    assert_eq!(fs.grants()[0].sandbox_path(), documents.as_path());
    assert_eq!(fs.grants()[0].access(), AccessMode::ReadOnly);
}

#[test]
fn home_expands_to_children_when_project_lives_under_home() {
    let tree = TestTree::new("home-expands");
    let home = tree.path("home/user");
    let docs = home.join("Documents");
    let project = home.join("freebsd-flatpak-poc");
    fs::create_dir_all(&docs).unwrap();
    fs::create_dir_all(project.join("runtime/chroots/org.example.App")).unwrap();
    let fs = HostFilesystem::from_metadata(
        &metadata("home;"),
        "user",
        &home,
        &project,
        &project.join("runtime/chroots/org.example.App"),
    )
    .unwrap();

    assert!(fs.grants().iter().any(|grant| {
        grant.host_path() == docs.as_path()
            && grant.sandbox_path() == Path::new("/home/user/Documents")
    }));
    assert!(!fs
        .grants()
        .iter()
        .any(|grant| grant.host_path() == project.as_path()));
}

#[test]
fn overlapping_permissions_keep_more_permissive_access() {
    let tree = TestTree::new("overlap");
    let home = tree.path("home/user");
    let documents = home.join("Documents");
    fs::create_dir_all(&documents).unwrap();
    let fs = HostFilesystem::from_metadata(
        &metadata("xdg-documents:ro;xdg-documents;"),
        "user",
        &home,
        &tree.path("project"),
        &tree.path("project/runtime/chroots/org.example.App"),
    )
    .unwrap();
    assert_eq!(fs.grants().len(), 1);
    assert_eq!(fs.grants()[0].access(), AccessMode::ReadWrite);
}
