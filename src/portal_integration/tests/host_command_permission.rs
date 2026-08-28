use super::*;
use crate::sandbox::static_overrides::effective_metadata;
use std::fs;

const APP_ID: &str = "org.example.App";

fn effective_gate(name: &str, base: &str, global: Option<&str>, app: Option<&str>) -> bool {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-host-command-permission-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let metadata_path = root.join("app-metadata");
    let overrides = root.join("overrides");
    fs::create_dir_all(&overrides).unwrap();
    fs::write(&metadata_path, base).unwrap();
    if let Some(global) = global {
        fs::write(overrides.join("global"), global).unwrap();
    }
    if let Some(app) = app {
        fs::write(overrides.join(APP_ID), app).unwrap();
    }
    let metadata = effective_metadata(&metadata_path, &overrides, APP_ID).unwrap();
    let allowed = metadata_allows_host_command(&metadata);
    fs::remove_dir_all(root).unwrap();
    allowed
}

#[test]
fn host_command_requires_an_exact_talk_policy() {
    assert!(metadata_allows_host_command(
        "[Session Bus Policy]\norg.freedesktop.Flatpak=talk\n"
    ));

    for metadata in [
        "[Session Bus Policy]\norg.freedesktop.Flatpak=none\n",
        "[Session Bus Policy]\norg.freedesktop.Flatpak=own\n",
        "[Session Bus Policy]\norg.freedesktop.Flatpak.*=talk\n",
        "[System Bus Policy]\norg.freedesktop.Flatpak=talk\n",
        "[Session Bus Policy]\norg.freedesktop.Flatpak=talk-more\n",
        "[Application]\nname=org.example.App\n",
    ] {
        assert!(!metadata_allows_host_command(metadata), "{metadata:?}");
    }
}

#[test]
fn effective_app_override_can_revoke_base_host_command_access() {
    assert!(!effective_gate(
        "revoke",
        "[Session Bus Policy]\norg.freedesktop.Flatpak=talk\n",
        None,
        Some("[Session Bus Policy]\norg.freedesktop.Flatpak=none\n"),
    ));
}

#[test]
fn effective_app_override_can_grant_host_command_access() {
    assert!(effective_gate(
        "grant",
        "[Application]\nname=org.example.App\n",
        None,
        Some("[Session Bus Policy]\norg.freedesktop.Flatpak=talk\n"),
    ));
}

#[test]
fn application_override_wins_over_a_global_host_command_grant() {
    assert!(!effective_gate(
        "precedence",
        "[Application]\nname=org.example.App\n",
        Some("[Session Bus Policy]\norg.freedesktop.Flatpak=talk\n"),
        Some("[Session Bus Policy]\norg.freedesktop.Flatpak=none\n"),
    ));
}
