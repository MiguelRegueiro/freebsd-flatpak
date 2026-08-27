use super::{authorized_grant_paths, AccessMode, HostFilesystem, XdgUserDirs};
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
            "freebsd-flatpak-{name}-{}-{id}",
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

fn filesystem_with_user_dirs(
    tree: &TestTree,
    filesystems: &str,
    user_dirs: Option<&str>,
) -> HostFilesystem {
    let home = tree.path("home/user");
    let config_home = tree.path("host-config");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&config_home).unwrap();
    if let Some(user_dirs) = user_dirs {
        fs::write(config_home.join("user-dirs.dirs"), user_dirs).unwrap();
    }
    HostFilesystem::from_metadata_with_xdg_dirs(
        &metadata(filesystems),
        "user",
        &home,
        &tree.path("project"),
        &tree.path("project/runtime/chroots/org.example.App"),
        Vec::new(),
        XdgUserDirs::load_from_config_home(&home, &config_home),
    )
    .unwrap()
}

fn generated_user_dirs(tree: &TestTree, filesystem: &HostFilesystem) -> String {
    let config_home = tree.path("sandbox-config");
    filesystem.write_xdg_user_dirs_config(&config_home).unwrap();
    fs::read_to_string(config_home.join("user-dirs.dirs")).unwrap()
}

#[test]
fn all_xdg_user_directory_permissions_are_published_consistently() {
    let tree = TestTree::new("all-xdg-user-dirs");
    let names = [
        ("DESKTOP", "Desk"),
        ("DOCUMENTS", "Docs"),
        ("DOWNLOAD", "Incoming"),
        ("MUSIC", "Audio"),
        ("PICTURES", "Images"),
        ("PUBLICSHARE", "Shared"),
        ("TEMPLATES", "Patterns"),
        ("VIDEOS", "Movies"),
    ];
    let mut config = String::new();
    for (key, directory) in names {
        fs::create_dir_all(tree.path(&format!("home/user/{directory}"))).unwrap();
        config.push_str(&format!("XDG_{key}_DIR=\"$HOME/{directory}\"\n"));
    }
    let filesystem = filesystem_with_user_dirs(
        &tree,
        "xdg-desktop;xdg-documents;xdg-download;xdg-music;xdg-pictures;xdg-public-share;xdg-templates;xdg-videos;",
        Some(&config),
    );
    let generated = generated_user_dirs(&tree, &filesystem);

    assert_eq!(filesystem.grants().len(), 8);
    assert_eq!(filesystem.user_dir_env().len(), 8);
    for (key, directory) in names {
        assert!(generated.contains(&format!(
            "XDG_{key}_DIR=\"{}\"",
            tree.path(&format!("home/user/{directory}")).display()
        )));
    }
}

#[test]
fn missing_user_dirs_config_uses_xdg_default_download_path() {
    let tree = TestTree::new("missing-user-dirs");
    let download = tree.path("home/user/Downloads");
    fs::create_dir_all(&download).unwrap();
    let filesystem = filesystem_with_user_dirs(&tree, "xdg-download;", None);

    assert_eq!(filesystem.grants()[0].host_path(), download);
    assert!(generated_user_dirs(&tree, &filesystem)
        .contains(&format!("XDG_DOWNLOAD_DIR=\"{}\"", download.display())));
}

#[test]
fn ungranted_xdg_directory_is_not_published_or_exposed() {
    let tree = TestTree::new("ungranted-xdg-download");
    let download = tree.path("home/user/Private Downloads");
    fs::create_dir_all(&download).unwrap();
    let filesystem = filesystem_with_user_dirs(
        &tree,
        "",
        Some("XDG_DOWNLOAD_DIR=\"$HOME/Private Downloads\"\n"),
    );
    let generated = generated_user_dirs(&tree, &filesystem);

    assert!(filesystem.grants().is_empty());
    assert!(filesystem.user_dir_env().is_empty());
    assert!(!generated.contains("XDG_DOWNLOAD_DIR="));
    assert!(!generated.contains(&download.display().to_string()));
}

#[test]
fn xdg_directory_configured_as_home_is_disabled() {
    let tree = TestTree::new("disabled-xdg-download");
    let filesystem =
        filesystem_with_user_dirs(&tree, "xdg-download;", Some("XDG_DOWNLOAD_DIR=\"$HOME\"\n"));

    assert!(filesystem.grants().is_empty());
    assert!(filesystem
        .warnings()
        .iter()
        .any(|warning| warning == "disabled XDG filesystem permission: xdg-download"));
    assert!(!generated_user_dirs(&tree, &filesystem).contains("XDG_DOWNLOAD_DIR="));
}

#[test]
fn home_expands_to_children_when_project_lives_under_home() {
    let tree = TestTree::new("home-expands");
    let home = tree.path("home/user");
    let docs = home.join("Documents");
    let project = home.join("freebsd-flatpak");
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

#[test]
fn host_grants_include_only_component_bounded_nested_mount_points() {
    let roots = vec![PathBuf::from("/home/foo")];
    let mounts = vec![
        PathBuf::from("/home"),
        PathBuf::from("/home/foo"),
        PathBuf::from("/home/foo/archive"),
        PathBuf::from("/home/foo/../escape"),
        PathBuf::from("/home/foobar"),
        PathBuf::from("home/foo/relative"),
        PathBuf::from("/unrelated"),
    ];

    assert_eq!(
        authorized_grant_paths(&roots, &mounts),
        vec![
            PathBuf::from("/home/foo"),
            PathBuf::from("/home/foo/archive"),
        ]
    );
}

#[test]
fn host_grants_are_deduplicated_and_ordered_parent_before_child() {
    let roots = vec![PathBuf::from("/home"), PathBuf::from("/mnt")];
    let mounts = vec![
        PathBuf::from("/"),
        PathBuf::from("/home"),
        PathBuf::from("/home/regueiro"),
        PathBuf::from("/home/regueiro"),
        PathBuf::from("/home/regueiro/archive"),
        PathBuf::from("/unrelated"),
    ];

    assert_eq!(
        authorized_grant_paths(&roots, &mounts),
        vec![
            PathBuf::from("/home"),
            PathBuf::from("/mnt"),
            PathBuf::from("/home/regueiro"),
            PathBuf::from("/home/regueiro/archive"),
        ]
    );
}

#[test]
fn host_read_only_permission_keeps_every_subordinate_grant_read_only() {
    let tree = TestTree::new("host-ro");
    let home = tree.path("home/user");
    fs::create_dir_all(&home).unwrap();
    let fs = HostFilesystem::from_metadata(
        &metadata("host:ro;"),
        "user",
        &home,
        &tree.path("project"),
        &tree.path("project/runtime/chroots/org.example.App"),
    )
    .unwrap();

    assert!(!fs.grants().is_empty());
    assert!(fs
        .grants()
        .iter()
        .all(|grant| grant.access() == AccessMode::ReadOnly));
}

#[test]
fn narrow_read_only_grant_projects_only_its_subordinate_mounts() {
    let tree = TestTree::new("narrow-submount-ro");
    let home = tree.path("home/user");
    let granted = home.join("foo");
    let nested = granted.join("archive");
    let lookalike = home.join("foobar");
    fs::create_dir_all(&nested).unwrap();
    fs::create_dir_all(&lookalike).unwrap();
    let fs = HostFilesystem::from_metadata_with_mount_points(
        &metadata("home/foo:ro;"),
        "user",
        &home,
        &tree.path("project"),
        &tree.path("project/runtime/chroots/org.example.App"),
        vec![granted.clone(), nested.clone(), lookalike],
    )
    .unwrap();

    assert_eq!(fs.grants().len(), 2);
    assert_eq!(fs.grants()[0].host_path(), granted);
    assert_eq!(fs.grants()[0].sandbox_path(), Path::new("/home/user/foo"));
    assert_eq!(fs.grants()[0].access(), AccessMode::ReadOnly);
    assert_eq!(fs.grants()[1].host_path(), nested);
    assert_eq!(
        fs.grants()[1].sandbox_path(),
        Path::new("/home/user/foo/archive")
    );
    assert_eq!(fs.grants()[1].access(), AccessMode::ReadOnly);
}

#[test]
fn overlapping_parent_and_child_grants_keep_explicit_access_and_mount_order() {
    let tree = TestTree::new("parent-child-access");
    let home = tree.path("home/user");
    let documents = home.join("Documents");
    fs::create_dir_all(&documents).unwrap();
    let fs = HostFilesystem::from_metadata(
        &metadata("home;home/Documents:ro;"),
        "user",
        &home,
        &tree.path("project"),
        &tree.path("project/runtime/chroots/org.example.App"),
    )
    .unwrap();

    assert_eq!(fs.grants().len(), 2);
    assert_eq!(fs.grants()[0].sandbox_path(), Path::new("/home/user"));
    assert_eq!(fs.grants()[0].access(), AccessMode::ReadWrite);
    assert_eq!(
        fs.grants()[1].sandbox_path(),
        Path::new("/home/user/Documents")
    );
    assert_eq!(fs.grants()[1].access(), AccessMode::ReadOnly);
}

#[test]
fn create_permission_materializes_missing_protected_subtree_and_keeps_mode() {
    let tree = TestTree::new("protected-create");
    let home = tree.path("home/user");
    fs::create_dir_all(&home).unwrap();
    let created = home.join(".local/share/flatpak/repo/objects");
    let filesystem = HostFilesystem::from_metadata(
        &metadata("~/.local/share/flatpak/repo/objects:create;"),
        "user",
        &home,
        &tree.path("project"),
        &tree.path("sandbox"),
    )
    .unwrap();

    assert!(created.is_dir());
    assert_eq!(filesystem.grants().len(), 1);
    assert_eq!(
        filesystem.grants()[0].sandbox_path(),
        Path::new("/home/user/.local/share/flatpak/repo/objects")
    );
    assert_eq!(filesystem.grants()[0].access(), AccessMode::ReadWrite);
    assert_eq!(
        filesystem.grants()[0].source_permission(),
        "~/.local/share/flatpak/repo/objects:create"
    );
}

#[test]
fn missing_protected_subtree_without_create_is_not_granted() {
    let tree = TestTree::new("protected-missing");
    let home = tree.path("home/user");
    fs::create_dir_all(&home).unwrap();
    let filesystem = HostFilesystem::from_metadata(
        &metadata("~/.local/share/flatpak/missing:ro;"),
        "user",
        &home,
        &tree.path("project"),
        &tree.path("sandbox"),
    )
    .unwrap();

    assert!(filesystem.grants().is_empty());
    assert!(filesystem
        .warnings()
        .iter()
        .any(|warning| warning.contains("skipping missing host path")));
}

#[test]
fn read_only_parent_and_writable_child_keep_explicit_precedence() {
    let tree = TestTree::new("parent-ro-child-rw");
    let home = tree.path("home/user");
    let child = home.join(".var/app/org.example.Other/data");
    fs::create_dir_all(&child).unwrap();
    let filesystem = HostFilesystem::from_metadata(
        &metadata("~/.var/app/org.example.Other:ro;~/.var/app/org.example.Other/data:rw;"),
        "user",
        &home,
        &tree.path("project"),
        &tree.path("sandbox"),
    )
    .unwrap();

    assert_eq!(filesystem.grants().len(), 2);
    assert_eq!(filesystem.grants()[0].access(), AccessMode::ReadOnly);
    assert_eq!(filesystem.grants()[1].access(), AccessMode::ReadWrite);
    assert!(filesystem.grants()[1]
        .sandbox_path()
        .starts_with(filesystem.grants()[0].sandbox_path()));
}
