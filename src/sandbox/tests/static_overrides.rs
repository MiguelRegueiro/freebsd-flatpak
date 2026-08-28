use super::*;
use crate::flatpak_metadata::value;

const METADATA: &str = "[Application]\nname=org.example.App\ncommand=example\n\n[Context]\nshared=ipc;network;\nsockets=wayland;pulseaudio;\nfilesystems=home;xdg-download:ro;\n\n[Environment]\nKEEP=base\nREPLACE=base\n";

#[test]
fn global_and_application_overrides_merge_with_flatpak_precedence() {
    let global = "[Context]\nshared=!network;\nfilesystems=!home;xdg-documents:ro;\n\n[Environment]\nREPLACE=global\nGLOBAL=yes\n";
    let application = "[Context]\nshared=network;\nsockets=!pulseaudio;\nfilesystems=xdg-documents:create;\n\n[Environment]\nREPLACE=application\n";
    let effective =
        effective_metadata_from_sources(METADATA, Some(global), Some(application)).unwrap();

    assert_eq!(
        value(&effective, "Context", "shared").unwrap(),
        "ipc;network;"
    );
    assert_eq!(
        value(&effective, "Context", "sockets").unwrap(),
        "wayland;!pulseaudio;"
    );
    assert_eq!(
        value(&effective, "Context", "filesystems").unwrap(),
        "xdg-download:ro;!home;xdg-documents:create;"
    );
    assert_eq!(value(&effective, "Environment", "KEEP").unwrap(), "base");
    assert_eq!(value(&effective, "Environment", "GLOBAL").unwrap(), "yes");
    assert_eq!(
        value(&effective, "Environment", "REPLACE").unwrap(),
        "application"
    );
}

#[test]
fn missing_overrides_leave_metadata_semantically_unchanged() {
    let effective = effective_metadata_from_sources(METADATA, None, None).unwrap();
    assert_eq!(
        value(&effective, "Application", "name").unwrap(),
        "org.example.App"
    );
    assert_eq!(
        value(&effective, "Context", "filesystems").unwrap(),
        "home;xdg-download:ro;"
    );
}

#[test]
fn permission_checks_understand_modes_and_negation() {
    let metadata =
        "[Context]\nfilesystems=xdg-data/flatpak/app:ro;xdg-data/flatpak/overrides:create;!home;\n";
    assert!(permission_enabled(
        metadata,
        "Context",
        "filesystems",
        "xdg-data/flatpak/app"
    ));
    assert!(permission_enabled(
        metadata,
        "Context",
        "filesystems",
        "xdg-data/flatpak/overrides"
    ));
    assert!(!permission_enabled(
        metadata,
        "Context",
        "filesystems",
        "home"
    ));
}
