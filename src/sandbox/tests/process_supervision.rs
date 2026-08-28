use super::*;
use std::fs;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[test]
fn termination_kills_and_reaps_tracked_and_detached_processes() {
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
