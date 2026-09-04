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
    assert_eq!(attempts.iter().filter(|force| !**force).count(), 1);
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
fn live_nested_root_is_skipped_by_startup_recovery() {
    let base = std::env::temp_dir().join(format!(
        "freebsd-flatpak-live-nested-recovery-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&base);
    let paths = Installation::for_test(&base);
    let parent = paths.chroots().join("org.example.App/parent");
    let nested = paths.chroots().join("org.example.App/parent-nested-1");
    fs::create_dir_all(&parent).unwrap();
    fs::create_dir_all(&nested).unwrap();
    let parent_record = state::write_run_record(
        &paths,
        "org.example.App",
        "parent",
        &parent,
        std::process::id(),
        0,
    )
    .unwrap();
    let nested_record = state::write_nested_run_record(
        &paths,
        "org.example.App",
        "parent-nested-1",
        &nested,
        &parent,
        std::process::id(),
    )
    .unwrap();

    recover_stale_mounts(&paths).unwrap();
    assert!(parent.exists());
    assert!(nested.exists());
    assert!(parent_record.exists());
    assert!(nested_record.exists());
    let active = active_run_roots(&paths).unwrap();
    assert!(belongs_to_any_root(&parent.join("usr"), &active));
    assert!(belongs_to_any_root(
        &nested.join(".freebsd-flatpak-mount-sources/0"),
        &active
    ));

    state::remove_run_record(&nested_record).unwrap();
    state::remove_run_record(&parent_record).unwrap();
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn abandoned_nested_root_is_recovered_before_parent() {
    let base = std::env::temp_dir().join(format!(
        "freebsd-flatpak-abandoned-nested-recovery-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&base);
    let paths = Installation::for_test(&base);
    let parent = paths.chroots().join("org.example.App/parent");
    let nested = paths.chroots().join("org.example.App/parent-nested-1");
    fs::create_dir_all(&parent).unwrap();
    fs::create_dir_all(&nested).unwrap();
    state::write_run_record(&paths, "org.example.App", "parent", &parent, 999_999, 0).unwrap();
    state::write_nested_run_record(
        &paths,
        "org.example.App",
        "parent-nested-1",
        &nested,
        &parent,
        999_999,
    )
    .unwrap();

    let ordered =
        order_run_records_for_recovery(state::read_sandbox_ownership_records(&paths).unwrap());
    let roots = ordered
        .iter()
        .filter_map(|record| record.get("root").map(PathBuf::from))
        .collect::<Vec<_>>();
    assert_eq!(roots, vec![nested.clone(), parent.clone()]);

    recover_stale_mounts(&paths).unwrap();
    assert!(!nested.exists());
    assert!(!parent.exists());
    assert!(state::read_sandbox_ownership_records(&paths)
        .unwrap()
        .is_empty());
    fs::remove_dir_all(base).unwrap();
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

#[test]
fn dead_launcher_with_live_child_is_preserved_until_the_child_exits() {
    let base = std::env::temp_dir().join(format!(
        "freebsd-flatpak-live-child-recovery-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&base);
    let paths = Installation::for_test(&base);
    let root = paths.chroots().join("org.example.App/dead-launcher");
    fs::create_dir_all(&root).unwrap();
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .current_dir(&root)
        .spawn()
        .unwrap();
    let record = state::write_run_record(
        &paths,
        "org.example.App",
        "dead-launcher",
        &root,
        i32::MAX as u32,
        child.id(),
    )
    .unwrap();

    recover_stale_mounts(&paths).unwrap();
    assert!(root.exists());
    assert!(record.exists());
    assert!(child.try_wait().unwrap().is_none());

    child.kill().unwrap();
    child.wait().unwrap();
    recover_stale_mounts(&paths).unwrap();
    assert!(!root.exists());
    assert!(!record.exists());
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn live_nested_process_preserves_nested_and_parent_ownership() {
    let parent = PathBuf::from("/chroots/org.example.App/parent");
    let nested = PathBuf::from("/chroots/org.example.App/parent-nested-1");
    let records = vec![
        BTreeMap::from([
            ("root".to_string(), parent.display().to_string()),
            ("launcher_pid".to_string(), i32::MAX.to_string()),
        ]),
        BTreeMap::from([
            ("root".to_string(), nested.display().to_string()),
            ("parent_root".to_string(), parent.display().to_string()),
            ("launcher_pid".to_string(), i32::MAX.to_string()),
        ]),
    ];
    let processes = SandboxProcessSnapshot::for_test(vec![(42, nested.join("work"))]);

    let active = active_roots_from_records(&records, &processes).unwrap();
    assert!(active.contains(&nested));
    assert!(active.contains(&parent));
}

#[test]
fn stale_nested_mount_tree_is_ordered_before_its_sources_and_parent() {
    let base = std::env::temp_dir().join(format!(
        "freebsd-flatpak-nested-mount-order-{}",
        std::process::id()
    ));
    let chroots = base.join("chroots");
    let parent = chroots.join("org.example.App/parent");
    let nested = chroots.join("org.example.App/nested");
    fs::create_dir_all(parent.join("usr")).unwrap();
    fs::create_dir_all(nested.join(".freebsd-flatpak-mount-sources/0")).unwrap();
    fs::create_dir_all(nested.join("usr")).unwrap();
    fs::write(parent.join(".flatpak-info"), "[Instance]\n").unwrap();
    fs::write(nested.join(".flatpak-info"), "[Instance]\n").unwrap();

    let mounts = vec![
        MountInfo {
            source: PathBuf::from("/runtime"),
            mountpoint: parent.join("usr"),
            options: "read-only".to_string(),
        },
        MountInfo {
            source: parent.join("usr"),
            mountpoint: nested.join(".freebsd-flatpak-mount-sources/0"),
            options: "read-only".to_string(),
        },
        MountInfo {
            source: nested.join(".freebsd-flatpak-mount-sources/0"),
            mountpoint: nested.join("usr"),
            options: "read-only".to_string(),
        },
    ];
    let roots = BTreeSet::from([parent.clone(), nested.clone()]);
    let root_order = order_sandbox_roots_for_recovery(&chroots, roots, &[], &mounts);
    assert_eq!(root_order, vec![nested.clone(), parent.clone()]);

    let nested_mount_order = order_mounts_for_recovery(mounts[1..].to_vec());
    assert_eq!(nested_mount_order[0].mountpoint, nested.join("usr"));
    assert_eq!(
        nested_mount_order[1].mountpoint,
        nested.join(".freebsd-flatpak-mount-sources/0")
    );

    let processes = SandboxProcessSnapshot::for_test(vec![(42, nested.join("work"))]);
    let roots = BTreeSet::from([parent.clone(), nested.clone()]);
    let active = active_sandbox_roots(&chroots, &[], &mounts, &roots, &processes).unwrap();
    assert!(active.contains(&nested));
    assert!(active.contains(&parent));

    fs::remove_dir_all(base).unwrap();
}

#[test]
fn stale_mounts_are_sorted_deepest_first() {
    let mut mounts = vec![
        PathBuf::from("/sandbox/usr"),
        PathBuf::from("/sandbox/run/user/1000/doc"),
        PathBuf::from("/sandbox/run"),
        PathBuf::from("/sandbox/usr/lib/extensions"),
    ];
    sort_mountpoints_deepest_first(&mut mounts);

    let positions = mounts
        .iter()
        .enumerate()
        .map(|(index, path)| (path.clone(), index))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert!(
        positions[&PathBuf::from("/sandbox/run/user/1000/doc")]
            < positions[&PathBuf::from("/sandbox/run")]
    );
    assert!(
        positions[&PathBuf::from("/sandbox/usr/lib/extensions")]
            < positions[&PathBuf::from("/sandbox/usr")]
    );
}
