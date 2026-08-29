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
