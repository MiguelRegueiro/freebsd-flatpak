use super::*;
use crate::sandbox::chroot_instance::nested_mount_plan;

#[test]
fn spawn_supports_exposed_pids_and_start_notification() {
    let nested_flags = 0x04 | SPAWN_EXPOSE_PIDS | SPAWN_NOTIFY_START;

    assert!(spawn_flags_supported(nested_flags));
    assert!(!spawn_flags_supported(1 << 7));
}

#[test]
fn spawn_started_maps_external_to_sandbox_pid() {
    let pid = 1234;
    let relative_pid = 5678;
    let payload = spawn_started_payload(pid, relative_pid);

    assert_eq!(u32::from_be_bytes(payload[..4].try_into().unwrap()), pid);
    assert_eq!(
        u32::from_be_bytes(payload[4..].try_into().unwrap()),
        relative_pid
    );
}

#[test]
fn notify_start_without_expose_pids_reports_zero_relpid() {
    assert_eq!(spawn_started_relative_pid(false, 5678), 0);
}

#[test]
fn expose_pids_reports_started_relative_pid() {
    assert_eq!(spawn_started_relative_pid(true, 5678), 5678);
}

#[test]
fn sandbox_mount_plan_drops_parent_extra_filesystem_grants() {
    let runtime = OwnedMount {
        path: PathBuf::from("/sandbox/usr"),
        read_only: true,
    };
    let host_grant = OwnedMount {
        path: PathBuf::from("/sandbox/home/user"),
        read_only: false,
    };

    assert_eq!(
        nested_mount_plan(
            &[runtime.clone(), host_grant.clone()],
            std::slice::from_ref(&host_grant.path),
        )
        .iter()
        .map(|mount| mount.path.as_path())
        .collect::<Vec<_>>(),
        vec![runtime.path.as_path()]
    );
}
#[test]
fn readiness_notification_is_required_before_spawn_started() {
    let (read, write) = readiness_pipe(3).unwrap();
    assert_eq!(
        unsafe { libc::write(write.as_raw_fd(), 5678u32.to_be_bytes().as_ptr().cast(), 4,) },
        4
    );

    assert_eq!(wait_for_spawn_started(read.as_raw_fd()).unwrap(), 5678);
}

#[test]
fn nested_spawn_metadata_marks_the_child_restricted() {
    let info = "[Application]\nname=org.example.App\n\n[Instance]\ninstance-id=one\n";
    let nested = restricted_flatpak_info(info);

    assert!(nested.contains("[Instance]\nsandbox=true\ninstance-id=one\n"));
    assert_eq!(restricted_flatpak_info(&nested), nested);
}

#[test]
fn nested_spawn_ignores_parent_staging_and_preserves_mountpoints() {
    assert!(is_internal_mount_staging(Path::new(
        ".freebsd-flatpak-mount-sources"
    )));
    assert!(is_internal_mount_staging(Path::new(
        ".freebsd-flatpak-mount-sources/0"
    )));
    assert!(!is_internal_mount_staging(Path::new("home/regueiro")));

    let root = std::env::temp_dir().join(format!(
        "ffp-nested-mountpoints-test-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let source = root.join("source");
    let target = root.join("target");
    fs::create_dir_all(source.join("usr")).unwrap();
    fs::write(source.join("usr/parent-content"), b"hidden").unwrap();
    fs::create_dir(&target).unwrap();

    copy_unmounted_tree(
        &source,
        &target,
        &[OwnedMount {
            path: source.join("usr"),
            read_only: true,
        }],
    )
    .unwrap();

    assert!(target.join("usr").is_dir());
    assert!(!target.join("usr/parent-content").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn accepted_notify_start_emits_started_even_when_pid_mapping_fails() {
    let mut sockets = [-1; 2];
    assert_eq!(
        unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
                0,
                sockets.as_mut_ptr(),
            )
        },
        0
    );
    let broker = unsafe { OwnedFd::from_raw_fd(sockets[0]) };
    let portal = unsafe { OwnedFd::from_raw_fd(sockets[1]) };
    let (readiness, write) = readiness_pipe(3).unwrap();
    drop(write);

    let accepted = 1234u32.to_be_bytes();
    assert!(unsafe {
        send_frame(
            broker.as_raw_fd(),
            &frame(SPAWN_ACCEPTED, 1, &accepted, 0),
            &accepted,
        )
    });
    assert!(unsafe {
        complete_spawn_start_notification(broker.as_raw_fd(), 1, 1234, Some(&readiness), true)
    });

    let mut packet = [0; 28];
    assert_eq!(
        unsafe { libc::recv(portal.as_raw_fd(), packet.as_mut_ptr().cast(), 24, 0) },
        24
    );
    assert_eq!(
        parse_frame(&packet[..24]).unwrap(),
        (SPAWN_ACCEPTED, 1, 4, 0)
    );
    assert_eq!(
        unsafe {
            libc::recv(
                portal.as_raw_fd(),
                packet.as_mut_ptr().cast(),
                packet.len(),
                0,
            )
        },
        packet.len() as isize
    );
    assert_eq!(parse_frame(&packet).unwrap(), (SPAWN_STARTED, 1, 8, 0));
    assert_eq!(u32::from_be_bytes(packet[20..24].try_into().unwrap()), 1234);
    assert_eq!(u32::from_be_bytes(packet[24..28].try_into().unwrap()), 0);
}
