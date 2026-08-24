use super::*;
use crate::sandbox::launch_application::FlatpakApp;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static TEST_ID: AtomicUsize = AtomicUsize::new(0);

fn app_with_metadata(metadata: &str) -> FlatpakApp {
    let id = TEST_ID.fetch_add(1, Ordering::SeqCst);
    let app_dir = std::env::temp_dir().join(format!(
        "freebsd-flatpak-entry-test-{}-{id}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&app_dir);
    fs::create_dir_all(&app_dir).unwrap();
    fs::write(app_dir.join("metadata"), metadata).unwrap();
    FlatpakApp {
        app_id: "org.example.App".into(),
        app_dir,
        runtime_ref: "org.freedesktop.Platform/x86_64/25.08".into(),
        runtime_dir: PathBuf::from("/runtime"),
        command: "app".into(),
        args: Vec::new(),
    }
}

#[test]
fn electron_base_app_gets_wayland_ozone_arg() {
    let app = app_with_metadata(
        "[Application]\nbase=app/org.electronjs.Electron2.BaseApp/x86_64/25.08\n\n[Context]\nsockets=wayland;x11;\n",
    );

    assert_eq!(
        compatibility_args(&app, &[]).unwrap(),
        vec!["--ozone-platform=wayland"]
    );
}

#[test]
fn explicit_ozone_arg_is_preserved() {
    let app = app_with_metadata(
        "[Application]\nbase=app/org.electronjs.Electron2.BaseApp/x86_64/25.08\n\n[Context]\nsockets=wayland;x11;\n",
    );

    assert!(
        compatibility_args(&app, &["--ozone-platform=x11".to_string()])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn non_electron_app_gets_no_ozone_arg() {
    let app = app_with_metadata(
        "[Application]\nbase=app/org.gnome.Platform/x86_64/49\n\n[Context]\nsockets=wayland;\n",
    );

    assert!(compatibility_args(&app, &[]).unwrap().is_empty());
}

#[test]
fn linux_elf_entry_is_executed_directly() {
    let app = app_with_metadata("[Application]\n");
    fs::create_dir_all(app.app_dir.join("files/bin")).unwrap();
    fs::write(
        app.app_dir.join("files/app"),
        b"\x7fELF\x02\x01\x01\x00 test executable",
    )
    .unwrap();
    symlink("/app/app", app.app_dir.join("files/bin/app")).unwrap();

    let entry = resolve_entry(&app).unwrap();
    let mut command = Command::new("chroot");
    entry.append_command_args(&mut command, &["--example".into()]);

    assert_eq!(entry.display(&[]), "/app/bin/app");
    assert_eq!(
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        vec!["/app/bin/app", "--example"]
    );
}
