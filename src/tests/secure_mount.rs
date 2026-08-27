use super::*;
use std::fs;
use std::os::unix::fs::symlink;
use std::os::unix::fs::MetadataExt;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEST: AtomicU64 = AtomicU64::new(0);

struct TestTree {
    root: std::path::PathBuf,
}

impl TestTree {
    fn new(label: &str) -> Self {
        let serial = NEXT_TEST.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "freebsd-flatpak-secure-mount-{label}-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        Self { root }
    }
}

impl Drop for TestTree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn descriptor_chase_creates_multiple_nested_directories() {
    let tree = TestTree::new("nested");
    let root = open_absolute_dir(&tree.root).unwrap();

    let target = chase_and_mkdir(
        &root,
        Path::new("parent/child/grandchild"),
        0o755,
        unsafe { libc::geteuid() },
        unsafe { libc::getegid() },
    )
    .unwrap();

    assert!(tree.root.join("parent/child/grandchild").is_dir());
    assert_eq!(
        FileIdentity::from_fd(target.as_raw_fd()).unwrap().inode,
        fs::metadata(tree.root.join("parent/child/grandchild"))
            .unwrap()
            .ino()
    );
}

#[test]
fn descriptor_chase_rejects_symlinks_for_every_redirect_shape() {
    let tree = TestTree::new("redirects");
    let outside = TestTree::new("outside");
    fs::create_dir_all(tree.root.join("source/child")).unwrap();
    fs::create_dir_all(tree.root.join("elsewhere")).unwrap();
    fs::create_dir_all(outside.root.join("destination")).unwrap();
    symlink("../source", tree.root.join("back-to-source")).unwrap();
    symlink("elsewhere", tree.root.join("inside")).unwrap();
    symlink(outside.root.join("destination"), tree.root.join("outside")).unwrap();
    let root = open_absolute_dir(&tree.root).unwrap();

    for path in [
        "back-to-source/child/mount",
        "inside/mount",
        "outside/mount",
    ] {
        assert!(chase_and_mkdir(
            &root,
            Path::new(path),
            0o755,
            unsafe { libc::geteuid() },
            unsafe { libc::getegid() }
        )
        .is_err());
    }

    assert!(!tree.root.join("source/child/mount").exists());
    assert!(!tree.root.join("elsewhere/mount").exists());
    assert!(!outside.root.join("destination/mount").exists());
}

#[test]
fn held_target_descriptor_survives_path_rename_without_re_resolving() {
    let tree = TestTree::new("rename");
    fs::create_dir(tree.root.join("parent")).unwrap();
    let root = open_absolute_dir(&tree.root).unwrap();
    let target = chase_and_mkdir(
        &root,
        Path::new("parent/target"),
        0o755,
        unsafe { libc::geteuid() },
        unsafe { libc::getegid() },
    )
    .unwrap();
    let identity = FileIdentity::from_fd(target.as_raw_fd()).unwrap();

    fs::rename(
        tree.root.join("parent/target"),
        tree.root.join("parent/renamed"),
    )
    .unwrap();
    fs::create_dir(tree.root.join("parent/target")).unwrap();

    assert_eq!(
        FileIdentity::from_fd(target.as_raw_fd()).unwrap().inode,
        identity.inode
    );
    assert_ne!(
        fs::metadata(tree.root.join("parent/target")).unwrap().ino(),
        identity.inode
    );
}

#[test]
fn absolute_anchor_rejects_symlink_components() {
    let tree = TestTree::new("absolute-symlink");
    fs::create_dir(tree.root.join("real")).unwrap();
    symlink("real", tree.root.join("link")).unwrap();

    assert!(open_absolute_dir(&tree.root.join("link")).is_err());
}
