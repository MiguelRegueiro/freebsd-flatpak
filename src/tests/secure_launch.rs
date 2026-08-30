use super::*;

#[test]
fn request_rejects_non_instance_roots() {
    let root = std::env::temp_dir().join(format!("ffp-secure-launch-{}", std::process::id()));
    let runtime = root.join("runtime");
    let candidate = runtime.join("chroots/app/instance/extra");
    fs::create_dir_all(&candidate).unwrap();
    fs::write(candidate.join(".flatpak-info"), "[Instance]\n").unwrap();
    let metadata = fs::metadata(&candidate).unwrap();
    let request = Request {
        root: candidate,
        runtime_root: runtime,
        root_device: metadata.dev(),
        root_inode: metadata.ino(),
        uid: unsafe { libc::getuid() },
        gid: unsafe { libc::getgid() },
        groups: Vec::new(),
        cwd: None,
        jail_mode: JailMode::Direct,
        mapped_fds: Vec::new(),
        environment: Vec::new(),
        argv: vec![OsString::from("/bin/true")],
    };
    assert!(request
        .validate(unsafe { libc::getuid() }, unsafe { libc::getgid() })
        .is_err());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn nested_lifecycle_transfers_owning_descriptor() {
    let (parent, child) = nested_jail_lifecycle_socket().unwrap();
    let owner: OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
    send_nested_jail_lifecycle(child.as_raw_fd(), 1234, owner.as_raw_fd()).unwrap();
    let (jail, received) = receive_nested_jail_lifecycle(parent.as_raw_fd()).unwrap();

    assert_eq!(jail, 1234);
    assert!(unsafe { libc::fcntl(received.as_raw_fd(), libc::F_GETFD) } >= 0);
}

fn serve_test_nested_request(fd: RawFd) -> Result<()> {
    let mut request = [0u8; 1];
    read_exact(fd, &mut request)?;
    write_all(fd, &[request[0].wrapping_add(1)])
}

#[test]
fn second_nested_request_is_serviced_while_first_worker_is_blocked() {
    let listener: OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
    let (first_daemon, first_client) = nested_jail_lifecycle_socket().unwrap();
    let first = fork_nested_request_worker_with(
        listener.as_raw_fd(),
        first_daemon,
        serve_test_nested_request,
    )
    .unwrap();
    let (second_daemon, second_client) = nested_jail_lifecycle_socket().unwrap();
    let second = fork_nested_request_worker_with(
        listener.as_raw_fd(),
        second_daemon,
        serve_test_nested_request,
    )
    .unwrap();

    write_all(second_client.as_raw_fd(), &[41]).unwrap();
    let mut response = [0u8; 1];
    read_exact(second_client.as_raw_fd(), &mut response).unwrap();
    assert_eq!(response, [42]);
    assert_ne!(first, second);
    assert_eq!(
        unsafe { libc::waitpid(first, std::ptr::null_mut(), libc::WNOHANG) },
        0
    );
    unsafe { libc::kill(first, libc::SIGKILL) };
    assert_eq!(
        unsafe { libc::waitpid(first, std::ptr::null_mut(), 0) },
        first
    );
    assert_eq!(
        unsafe { libc::waitpid(second, std::ptr::null_mut(), 0) },
        second
    );
    drop(first_client);
}

#[test]
fn nested_client_disconnect_uses_normal_shutdown_signal() {
    let (daemon, client) = nested_jail_lifecycle_socket().unwrap();
    let child = unsafe { libc::fork() };
    assert!(child >= 0);
    if child == 0 {
        unsafe {
            libc::close(daemon.as_raw_fd());
            libc::close(client.as_raw_fd());
            libc::setpgid(0, 0);
            loop {
                libc::pause();
            }
        }
    }
    unsafe { libc::setpgid(child, child) };
    drop(client);

    let status = wait_for_nested_child_or_client(child, daemon.as_raw_fd()).unwrap();
    assert!(libc::WIFSIGNALED(status));
    assert_eq!(libc::WTERMSIG(status), libc::SIGTERM);
}

#[test]
fn nested_client_forwards_watch_bus_signal_to_the_real_child_group() {
    let (daemon, client) = nested_jail_lifecycle_socket().unwrap();
    let proxy = unsafe { libc::fork() };
    assert!(proxy >= 0);
    if proxy == 0 {
        unsafe { libc::close(daemon.as_raw_fd()) };
        install_nested_client_signal_handlers(client.as_raw_fd());
        unsafe {
            loop {
                libc::pause();
            }
        }
    }

    let child = unsafe { libc::fork() };
    assert!(child >= 0);
    if child == 0 {
        unsafe {
            libc::close(daemon.as_raw_fd());
            libc::close(client.as_raw_fd());
            libc::setpgid(0, 0);
            loop {
                libc::pause();
            }
        }
    }
    unsafe { libc::setpgid(child, child) };
    assert_eq!(unsafe { libc::kill(proxy, libc::SIGINT) }, 0);
    let mut proxy_status = 0;
    assert_eq!(unsafe { libc::waitpid(proxy, &mut proxy_status, 0) }, proxy);
    assert!(libc::WIFEXITED(proxy_status));
    assert_eq!(libc::WEXITSTATUS(proxy_status), 128 + libc::SIGINT);
    drop(client);

    let status = wait_for_nested_child_or_client(child, daemon.as_raw_fd()).unwrap();
    assert!(libc::WIFSIGNALED(status));
    assert_eq!(libc::WTERMSIG(status), libc::SIGINT);
}
