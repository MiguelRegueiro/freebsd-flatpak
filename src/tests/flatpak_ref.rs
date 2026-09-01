use super::*;

#[test]
fn parses_flatpak_partial_ref_forms() {
    let cases = [
        ("org.example.Platform", None, None, None),
        ("org.example.Platform/x86_64", None, Some("x86_64"), None),
        ("org.example.Platform//50", None, None, Some("50")),
        (
            "runtime/org.example.Platform/x86_64/50",
            Some(RefKind::Runtime),
            Some("x86_64"),
            Some("50"),
        ),
        (
            "app/org.example.App//stable",
            Some(RefKind::App),
            None,
            Some("stable"),
        ),
    ];
    for (value, kind, arch, branch) in cases {
        let parsed = PartialRef::parse(value).unwrap();
        assert_eq!(parsed.kind, kind, "{value}");
        assert_eq!(parsed.arch.as_deref(), arch, "{value}");
        assert_eq!(parsed.branch.as_deref(), branch, "{value}");
    }
}

#[test]
fn partial_refs_match_only_specified_components() {
    let candidate = FlatpakRef::parse("runtime/org.example.Platform/x86_64/50").unwrap();
    for value in [
        "org.example.Platform",
        "org.example.Platform/x86_64",
        "org.example.Platform//50",
        "runtime/org.example.Platform//50",
    ] {
        assert!(
            PartialRef::parse(value).unwrap().matches(&candidate),
            "{value}"
        );
    }
    assert!(!PartialRef::parse("app/org.example.Platform")
        .unwrap()
        .matches(&candidate));
    assert!(!PartialRef::parse("org.example.Platform/aarch64")
        .unwrap()
        .matches(&candidate));
    assert!(!PartialRef::parse("org.example.Platform//49")
        .unwrap()
        .matches(&candidate));
}

#[test]
fn kind_filters_reject_conflicting_qualified_refs() {
    let runtime = PartialRef::parse("runtime/org.example.Platform").unwrap();
    assert_eq!(
        runtime.effective_kind(None).unwrap(),
        Some(RefKind::Runtime)
    );
    assert!(runtime.effective_kind(Some(RefKind::App)).is_err());
}

#[test]
fn rejects_malformed_partial_refs() {
    for value in [
        "runtime/",
        "org.example.Platform/x86_64/50/extra",
        "org.example.Platform/bad@arch/50",
        "org.example.Platform//bad branch",
    ] {
        assert!(PartialRef::parse(value).is_err(), "{value}");
    }
}
