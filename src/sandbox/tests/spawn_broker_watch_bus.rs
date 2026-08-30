use super::*;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

fn socket_pair() -> (OwnedFd, OwnedFd) {
    let mut sockets = [0; 2];
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
    unsafe {
        (
            OwnedFd::from_raw_fd(sockets[0]),
            OwnedFd::from_raw_fd(sockets[1]),
        )
    }
}

#[test]
fn watch_bus_termination_requires_the_matching_spawn_request() {
    let (matching_server, matching_client) = socket_pair();
    let header = frame(SPAWN_TERMINATE, 41, &[], 0);
    assert!(unsafe { send_frame(matching_client.as_raw_fd(), &header, &[]) });
    assert!(unsafe { watch_bus_termination_requested(matching_server.as_raw_fd(), 41) });

    let (other_server, other_client) = socket_pair();
    let header = frame(SPAWN_TERMINATE, 42, &[], 0);
    assert!(unsafe { send_frame(other_client.as_raw_fd(), &header, &[]) });
    assert!(!unsafe { watch_bus_termination_requested(other_server.as_raw_fd(), 41) });
}

#[test]
fn broker_shutdown_and_watch_bus_keep_their_original_signals() {
    let (shutdown_server, _shutdown_client) = socket_pair();
    assert_eq!(
        unsafe { spawn_termination_signal(shutdown_server.as_raw_fd(), 1, false, true) },
        Some(libc::SIGTERM)
    );

    let (watched_server, watched_client) = socket_pair();
    let header = frame(SPAWN_TERMINATE, 1, &[], 0);
    assert!(unsafe { send_frame(watched_client.as_raw_fd(), &header, &[]) });
    assert_eq!(
        unsafe { spawn_termination_signal(watched_server.as_raw_fd(), 1, true, false) },
        Some(libc::SIGINT)
    );
}

#[test]
fn watch_bus_spawn_requires_a_current_authenticated_owner() {
    let mut ordinary_resolved = false;
    assert_eq!(
        resolve_watch_bus_owner(false, 0, |_| {
            ordinary_resolved = true;
            Ok(Some(1u8))
        })
        .unwrap(),
        None
    );
    assert!(!ordinary_resolved);

    assert!(resolve_watch_bus_owner::<u8>(true, 0, |_| Ok(Some(1))).is_err());
    assert!(resolve_watch_bus_owner::<u8>(true, 41, |_| Ok(None)).is_err());
    assert_eq!(
        resolve_watch_bus_owner(true, 41, |_| Ok(Some(7u8))).unwrap(),
        Some(7)
    );
}
