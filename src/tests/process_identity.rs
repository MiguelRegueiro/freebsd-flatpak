use super::*;

#[test]
fn current_process_identity_round_trips() {
    let identity = ProcessIdentity::for_pid(std::process::id() as libc::pid_t)
        .unwrap()
        .unwrap();
    assert_eq!(
        ProcessIdentity::parse(&identity.to_string()),
        Some(identity)
    );
}

#[test]
fn nonexistent_process_has_no_identity() {
    assert_eq!(ProcessIdentity::for_pid(i32::MAX).unwrap(), None);
}
