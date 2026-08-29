use super::*;
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::fs;
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
#[test]
fn orphaned_regular_document_mounts_are_recovered_only_after_the_instance_is_gone() {
    let base = std::env::temp_dir().join(format!(
        "freebsd-flatpak-orphaned-document-{}",
        std::process::id()
    ));
    let chroots = base.join("chroots");
    let mountpoint = chroots.join("org.example.App/dead/run/user/1001/doc/grant/file");
    fs::create_dir_all(chroots.join("org.example.App")).unwrap();

    assert!(is_orphaned_regular_document_mount(&chroots, &mountpoint));
    fs::create_dir_all(chroots.join("org.example.App/dead")).unwrap();
    assert!(!is_orphaned_regular_document_mount(&chroots, &mountpoint));
    assert!(!is_orphaned_regular_document_mount(
        &chroots,
        &chroots.join("org.example.App/dead/run/user/1001/doc/grant/directory/nested")
    ));

    fs::remove_dir_all(base).unwrap();
}

#[test]
fn process_cleanup_signals_the_complete_snapshot_before_escalating() {
    let remaining = RefCell::new(BTreeSet::from([101, 202]));
    let signals = RefCell::new(Vec::new());

    let survivors = terminate_processes_with(
        || Ok(remaining.borrow().iter().copied().collect()),
        |pid, signal| {
            signals.borrow_mut().push((pid, signal));
            if signal == libc::SIGKILL {
                remaining.borrow_mut().remove(&pid);
            }
        },
        || {},
    )
    .unwrap();

    assert!(survivors.is_empty());
    let signals = signals.into_inner();
    assert!(signals.contains(&(101, libc::SIGTERM)));
    assert!(signals.contains(&(202, libc::SIGTERM)));
    assert!(signals.contains(&(101, libc::SIGKILL)));
    assert!(signals.contains(&(202, libc::SIGKILL)));
}

#[test]
fn process_cleanup_reports_processes_that_survive_sigkill() {
    let survivors = terminate_processes_with(|| Ok(vec![303]), |_, _| {}, || {}).unwrap();

    assert_eq!(survivors, vec![303]);
}
