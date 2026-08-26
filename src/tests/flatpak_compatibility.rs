use super::*;

#[test]
fn package_version_and_flatpak_compatibility_are_independent() {
    assert_eq!(FLATPAK_COMPATIBILITY_VERSION, "1.12.0");
    assert_eq!(env!("CARGO_PKG_VERSION"), "0.1.0");
    assert_ne!(FLATPAK_COMPATIBILITY_VERSION, env!("CARGO_PKG_VERSION"));
}

#[test]
fn requirements_at_or_below_the_advertised_level_need_no_diagnostic() {
    assert_eq!(
        required_version_diagnostic(
            "[Application]\nrequired-flatpak=0.10.0\n",
            "Application",
            "app/org.example.App/x86_64/stable",
        ),
        None
    );
    assert_eq!(
        required_version_diagnostic(
            "[Application]\nrequired-flatpak=1.12.0\n",
            "Application",
            "app/org.example.App/x86_64/stable",
        ),
        None
    );
}

#[test]
fn newer_required_series_produces_a_non_blocking_diagnostic() {
    let diagnostic = required_version_diagnostic(
        "[Application]\nrequired-flatpak=1.14.0\n",
        "Application",
        "app/org.example.App/x86_64/stable",
    )
    .unwrap();

    assert!(diagnostic.contains("required-flatpak=1.14.0"));
    assert!(diagnostic.contains("advertised compatibility level 1.12.0"));
    assert!(diagnostic.contains("attempting launch"));
}

#[test]
fn supports_upstream_alternative_backport_requirements() {
    assert_eq!(
        required_version_diagnostic(
            "[Runtime]\nrequired-flatpak=1.14.4;1.12.0;\n",
            "Runtime",
            "runtime/org.example.Platform/x86_64/stable",
        ),
        None
    );
}

#[test]
fn invalid_required_versions_produce_a_non_blocking_diagnostic() {
    let diagnostic = required_version_diagnostic(
        "[Application]\nrequired-flatpak=latest\n",
        "Application",
        "app/org.example.App/x86_64/stable",
    )
    .unwrap();

    assert!(diagnostic.contains("invalid required-flatpak value \"latest\""));
    assert!(diagnostic.contains("attempting launch"));
}
