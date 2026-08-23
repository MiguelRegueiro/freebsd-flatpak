use super::*;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static TEST_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_root(name: &str) -> PathBuf {
    let id = TEST_ID.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-poc-audio-{name}-{}-{id}",
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
    let _ = fs::remove_dir_all(root);
}
