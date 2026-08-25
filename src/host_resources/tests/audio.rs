use super::*;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static TEST_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_root(name: &str) -> PathBuf {
    let id = TEST_ID.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-audio-{name}-{}-{id}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn no_socket_metadata_has_no_audio_bridge() {
    let root = temp_root("none");
    let audio = HostAudio::from_metadata(
        "[Context]\nsockets=wayland;fallback-x11;\n",
        &root.join("run"),
        1001,
    );
    assert!(!audio.has_audio_bridge());
    assert!(audio.env().is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pulseaudio_socket_sets_pulse_server() {
    let root = temp_root("pulse");
    fs::create_dir_all(root.join("run/pulse")).unwrap();
    fs::write(root.join("run/pulse/native"), "").unwrap();
    let audio = HostAudio::from_metadata(
        "[Context]\nsockets=wayland;pulseaudio;\n",
        &root.join("run"),
        1001,
    );
    assert!(audio.has_audio_bridge());
    assert!(audio.env().contains(&(
        "PULSE_SERVER".to_string(),
        "unix:/run/user/1001/pulse/native".to_string()
    )));
    assert!(audio.env().contains(&(
        "PIPEWIRE_REMOTE".to_string(),
        UNAVAILABLE_PIPEWIRE_REMOTE.to_string()
    )));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pipewire_is_used_when_it_is_the_only_available_audio_transport() {
    let root = temp_root("pipewire");
    fs::create_dir_all(root.join("run")).unwrap();
    fs::write(root.join("run/pipewire-0"), "").unwrap();
    let audio = HostAudio::from_metadata(
        "[Context]\nsockets=wayland;pipewire;\n",
        &root.join("run"),
        1001,
    );

    assert!(audio.has_audio_bridge());
    assert!(audio
        .env()
        .contains(&("PIPEWIRE_REMOTE".to_string(), "pipewire-0".to_string())));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pulseaudio_takes_precedence_when_both_transports_are_available() {
    let root = temp_root("pulse-and-pipewire");
    fs::create_dir_all(root.join("run/pulse")).unwrap();
    fs::write(root.join("run/pulse/native"), "").unwrap();
    fs::write(root.join("run/pipewire-0"), "").unwrap();
    let audio = HostAudio::from_metadata(
        "[Context]\nsockets=pulseaudio;pipewire;\n",
        &root.join("run"),
        1001,
    );

    let env = audio.env();
    assert!(env.contains(&(
        "PULSE_SERVER".to_string(),
        "unix:/run/user/1001/pulse/native".to_string()
    )));
    assert!(env.contains(&(
        "PIPEWIRE_REMOTE".to_string(),
        UNAVAILABLE_PIPEWIRE_REMOTE.to_string()
    )));
    assert!(!env.contains(&("PIPEWIRE_REMOTE".to_string(), "pipewire-0".to_string())));
    let _ = fs::remove_dir_all(root);
}
