use super::*;

fn identity() -> SandboxIdentity {
    SandboxIdentity::new(
        1001,
        1002,
        "example".to_string(),
        Some("media".to_string()),
        "Example User".to_string(),
        PathBuf::from("/home/example"),
    )
    .unwrap()
}

#[test]
fn passwd_contains_only_the_current_and_unmapped_users() {
    assert_eq!(
        identity().passwd_contents(),
        "example:x:1001:1002:Example User:/home/example:/bin/sh\n\
nfsnobody:x:65534:65534:Unmapped user:/:/sbin/nologin\n"
    );
}

#[test]
fn resolved_group_contains_the_primary_and_unmapped_groups() {
    assert_eq!(
        identity().group_contents(),
        "media:x:1002:example\nnfsnobody:x:65534:\n"
    );
}

#[test]
fn unresolved_group_contains_only_the_unmapped_group() {
    let identity = SandboxIdentity::new(
        1001,
        1002,
        "example".to_string(),
        None,
        "Example User".to_string(),
        PathBuf::from("/home/example"),
    )
    .unwrap();

    assert_eq!(identity.group_contents(), "nfsnobody:x:65534:\n");
}

#[test]
fn empty_group_name_contains_only_the_unmapped_group() {
    let identity = SandboxIdentity::new(
        1001,
        1002,
        "example".to_string(),
        Some(String::new()),
        "Example User".to_string(),
        PathBuf::from("/home/example"),
    )
    .unwrap();

    assert_eq!(identity.group_contents(), "nfsnobody:x:65534:\n");
}

#[test]
fn identity_rejects_invalid_resolved_group_name() {
    let error = SandboxIdentity::new(
        1001,
        1002,
        "example".to_string(),
        Some("media:injected".to_string()),
        "Example User".to_string(),
        PathBuf::from("/home/example"),
    )
    .unwrap_err();

    assert!(error.to_string().contains("group name"));
}
#[test]
fn identity_rejects_fields_that_could_add_database_entries() {
    let error = SandboxIdentity::new(
        1001,
        1001,
        "example\nroot".to_string(),
        Some("example".to_string()),
        "Example User".to_string(),
        PathBuf::from("/home/example"),
    )
    .unwrap_err();

    assert!(error.to_string().contains("user name"));
}
