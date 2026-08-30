use super::*;
use std::fs;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;

fn reaper_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
}
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[test]
fn force_stop_rejects_a_reused_launcher_pid_identity() {
    let pid = std::process::id() as libc::pid_t;
    let current = ProcessIdentity::for_pid(pid).unwrap().unwrap();
    let stale = ProcessIdentity::parse("0:0").unwrap();
    assert_ne!(current, stale);
    assert_eq!(
        force_stop_launcher(pid, stale).unwrap(),
        ForceStopResult::Stale
    );
}

#[test]
fn termination_kills_and_reaps_tracked_and_detached_processes() {
    let _lock = reaper_lock();
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let test_dir = std::env::temp_dir().join(format!(
        "freebsd-flatpak-process-tree-{}-{suffix}",
        std::process::id()
    ));
    fs::create_dir(&test_dir).unwrap();
    let pid_file = test_dir.join("descendant.pid");

    let reaper = ProcessReaper::acquire().unwrap();
    let mut launcher = Command::new("daemon")
        .arg("-p")
        .arg(&pid_file)
        .args(["/bin/sh", "-c", "trap '' TERM; exec sleep 30"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let process_tree = reaper.track(launcher.id()).unwrap();
    assert!(launcher.wait().unwrap().success());

    let deadline = Instant::now() + Duration::from_secs(2);
    let descendant = loop {
        if let Ok(pid) = fs::read_to_string(&pid_file) {
            if let Ok(pid) = pid.trim().parse::<i32>() {
                break pid;
            }
        }
        assert!(
            Instant::now() < deadline,
            "daemon did not publish its descendant pid"
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert_ne!(unsafe { libc::getsid(descendant) }, unsafe {
        libc::getsid(0)
    });

    process_tree.wait_for_exit(&mut launcher, || true).unwrap();

    assert_ne!(unsafe { libc::kill(descendant, 0) }, 0);
    drop(process_tree);
    drop(reaper);

    let ready_file = test_dir.join("tracked-child.ready");
    let reaper = ProcessReaper::acquire().unwrap();
    let mut tracked_child = Command::new("/bin/sh")
        .args([
            "-c",
            &format!(
                "trap '' TERM; : > '{}'; exec sleep 30",
                ready_file.display()
            ),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let process_tree = reaper.track(tracked_child.id()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !ready_file.exists() {
        assert!(
            Instant::now() < deadline,
            "tracked child did not become ready"
        );
        thread::sleep(Duration::from_millis(20));
    }

    let status = process_tree
        .wait_for_exit(&mut tracked_child, || true)
        .unwrap();

    assert_eq!(status.signal(), Some(libc::SIGKILL));
    let _ = fs::remove_dir_all(test_dir);
}

#[test]
fn orphan_cleanup_waits_for_the_tracked_root_to_exit() {
    let _lock = reaper_lock();
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let pid_file = std::env::temp_dir().join(format!(
        "freebsd-flatpak-orphan-cleanup-{}-{suffix}.pid",
        std::process::id()
    ));
    let reaper = ProcessReaper::acquire().unwrap();
    let mut first = Command::new("/bin/sh")
        .args([
            "-c",
            &format!(
                "trap '' INT TERM; sleep 30 & echo $! > '{}'; wait",
                pid_file.display()
            ),
        ])
        .spawn()
        .unwrap();
    let mut second = Command::new("sleep").arg("30").spawn().unwrap();
    let first_tree = reaper.track(first.id()).unwrap();
    let second_tree = reaper.track(second.id()).unwrap();
    let first_subtree = reaper.subtree_for_descendant(first.id()).unwrap().unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    let descendant = loop {
        if let Ok(pid) = fs::read_to_string(&pid_file) {
            if let Ok(pid) = pid.trim().parse::<i32>() {
                break pid;
            }
        }
        assert!(Instant::now() < deadline, "child pid was not published");
        thread::sleep(Duration::from_millis(20));
    };
    assert!(!reaper
        .terminate_orphaned_subtree_with_signal(first_subtree, libc::SIGINT)
        .unwrap());
    assert_eq!(unsafe { libc::kill(first.id() as i32, 0) }, 0);
    assert_eq!(unsafe { libc::kill(descendant, 0) }, 0);

    assert_eq!(unsafe { libc::kill(first.id() as i32, libc::SIGKILL) }, 0);
    assert_eq!(first.wait().unwrap().signal(), Some(libc::SIGKILL));
    assert!(reaper
        .terminate_orphaned_subtree_with_signal(first_subtree, libc::SIGINT)
        .unwrap());
    assert_ne!(unsafe { libc::kill(descendant, 0) }, 0);
    drop(first_tree);
    assert_eq!(unsafe { libc::kill(second.id() as i32, 0) }, 0);
    let status = second_tree.wait_for_exit(&mut second, || true).unwrap();
    assert!(matches!(
        status.signal(),
        Some(libc::SIGTERM | libc::SIGKILL)
    ));
    let _ = fs::remove_file(pid_file);
    drop(reaper);
}
