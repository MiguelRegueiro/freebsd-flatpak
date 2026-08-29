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

#[test]
fn regular_file_target_creates_parents_but_keeps_final_component_a_file() {
    let tree = TestTree::new("regular-file-target");
    let root = open_absolute_dir(&tree.root).unwrap();
    let uid = unsafe { libc::geteuid() };
    let gid = unsafe { libc::getegid() };
    let (parent, name) = prepare_nullfs_parent(
        &root,
        Path::new("run/user/1001/doc/grant/document.txt"),
        uid,
        gid,
    )
    .unwrap();

    let target = open_or_create_regular_file(&parent, &name, uid, gid).unwrap();

    assert!(tree.root.join("run/user/1001/doc/grant").is_dir());
    assert!(tree
        .root
        .join("run/user/1001/doc/grant/document.txt")
        .is_file());
    assert_eq!(file_type(&target).unwrap(), libc::S_IFREG);
    assert!(validate_relative_mount_target(
        &root,
        Path::new("run/user/1001/doc/grant/document.txt")
    )
    .is_ok());
}

#[test]
fn regular_file_target_rejects_final_and_parent_symlinks() {
    let tree = TestTree::new("regular-file-symlink");
    let outside = TestTree::new("regular-file-outside");
    fs::create_dir_all(tree.root.join("safe")).unwrap();
    fs::write(outside.root.join("document.txt"), "outside").unwrap();
    symlink(
        outside.root.join("document.txt"),
        tree.root.join("safe/document.txt"),
    )
    .unwrap();
    symlink(outside.root.as_path(), tree.root.join("redirect")).unwrap();
    let root = open_absolute_dir(&tree.root).unwrap();
    let uid = unsafe { libc::geteuid() };
    let gid = unsafe { libc::getegid() };

    let (parent, name) =
        prepare_nullfs_parent(&root, Path::new("safe/document.txt"), uid, gid).unwrap();
    assert!(open_or_create_regular_file(&parent, &name, uid, gid).is_err());
    assert!(validate_relative_mount_target(&root, Path::new("safe/document.txt")).is_err());
    assert!(prepare_nullfs_parent(&root, Path::new("redirect/document.txt"), uid, gid).is_err());
    assert!(!outside.root.join("document.txt").is_dir());
}

#[test]
fn absolute_entry_accepts_regular_files_without_following_symlinks() {
    let tree = TestTree::new("absolute-regular-file");
    fs::write(tree.root.join("document.txt"), "document").unwrap();
    symlink("document.txt", tree.root.join("document-link")).unwrap();

    let file = open_absolute_entry(&tree.root.join("document.txt")).unwrap();
    assert_eq!(file_type(&file).unwrap(), libc::S_IFREG);
    assert!(open_absolute_entry(&tree.root.join("document-link")).is_err());
}

#[test]
fn getmntinfo_rejects_zero_count_and_null_table() {
    assert!(getmntinfo_mounts(0, std::ptr::null()).is_err());
    assert!(getmntinfo_mounts(1, std::ptr::null()).is_err());
}

#[test]
fn failed_parent_restoration_rolls_back_successful_regular_file_mount() {
    let restore_attempts = std::cell::Cell::new(0);
    let rollback_attempts = std::cell::Cell::new(0);

    let error = finish_regular_file_mount(
        Ok(()),
        || {
            let attempt = restore_attempts.get() + 1;
            restore_attempts.set(attempt);
            if attempt == 1 {
                bail!("injected parent restoration failure");
            }
            Ok(())
        },
        || {
            rollback_attempts.set(rollback_attempts.get() + 1);
            Ok(())
        },
    )
    .unwrap_err();

    assert!(format!("{error:#}").contains("mount was rolled back"));
    assert_eq!(rollback_attempts.get(), 1);
    assert_eq!(restore_attempts.get(), 2);
}

#[test]
fn failed_rollback_after_parent_restoration_failure_requires_tracked_cleanup() {
    let restore_attempts = std::cell::Cell::new(0);
    let rollback_attempts = std::cell::Cell::new(0);

    let completion = finish_regular_file_mount(
        Ok(()),
        || {
            restore_attempts.set(restore_attempts.get() + 1);
            bail!("injected parent restoration failure");
        },
        || {
            rollback_attempts.set(rollback_attempts.get() + 1);
            bail!("injected mount rollback failure");
        },
    )
    .unwrap();

    let RegularFileMountCompletion::NeedsTrackedCleanup(details) = completion else {
        panic!("a mount with failed rollback must remain tracked");
    };
    assert!(details.contains("rollback failed"));
    assert_eq!(rollback_attempts.get(), 1);
    assert_eq!(restore_attempts.get(), 2);
}
