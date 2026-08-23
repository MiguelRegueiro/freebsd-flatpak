use super::*;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
#[test]
fn already_gone_mount_cleanup_is_idempotent() {
    let attempts = Cell::new(0);
    for _ in 0..2 {
        unmount_mountpoint_with(
            Path::new("/sandbox/usr"),
            true,
            "test umount",
            |_| Ok(false),
            |_, _| {
                attempts.set(attempts.get() + 1);
                Ok(())
            },
            || {},
        )
        .unwrap();
    }
    assert_eq!(attempts.get(), 0);
}

#[test]
fn mount_disappearing_after_failed_unmount_is_not_retried_or_forced() {
    let mounted = Cell::new(true);
    let attempts = RefCell::new(Vec::new());
    unmount_mountpoint_with(
        Path::new("/sandbox/usr"),
        true,
        "test umount",
        |_| Ok(mounted.get()),
        |_, force| {
            attempts.borrow_mut().push(force);
            mounted.set(false);
            bail!("simulated race with another cleanup path")
        },
        || {},
    )
    .unwrap();
    assert_eq!(*attempts.borrow(), vec![false]);
}

#[test]
fn force_unmount_requires_a_still_mounted_owned_path() {
    let mounted = Cell::new(true);
    let attempts = RefCell::new(Vec::new());
    unmount_mountpoint_with(
        Path::new("/sandbox/usr"),
        true,
        "test umount",
        |_| Ok(mounted.get()),
        |_, force| {
            attempts.borrow_mut().push(force);
            if force {
                mounted.set(false);
                Ok(())
            } else {
                bail!("busy")
            }
        },
        || {},
    )
    .unwrap();
    let attempts = attempts.into_inner();
    assert_eq!(attempts.iter().filter(|force| !**force).count(), 8);
    assert_eq!(attempts.iter().filter(|force| **force).count(), 1);
}

#[test]
fn active_instance_roots_exclude_only_their_own_mounts_from_recovery() {
    let first = PathBuf::from("/chroots/org.example.App/first");
    let second = PathBuf::from("/chroots/org.example.App/second");
    let other = PathBuf::from("/chroots/org.example.Other/only");
    let active = vec![first.clone(), second.clone()];

    assert!(belongs_to_any_root(&first.join("usr"), &active));
    assert!(belongs_to_any_root(&second.join("proc"), &active));
    assert!(!belongs_to_any_root(&other.join("app"), &active));
}
