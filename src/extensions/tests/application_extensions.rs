use super::ensure_app_extensions;
use crate::installation::installation_paths::Installation;
use crate::sandbox::FlatpakApp;
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn installed_app_extensions_use_metadata_directories_and_subdirectories() {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-app-extensions-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let paths = Installation::for_test(&root);
    paths.ensure().unwrap();
    let app_dir = root.join("app");
    fs::create_dir_all(app_dir.join("files/lib/i386-linux-gnu/GL")).unwrap();
    fs::write(
        app_dir.join("metadata"),
        "[Application]\nname=org.example.App\nruntime=org.freedesktop.Platform/x86_64/25.08\n\n[Extension org.freedesktop.Platform.Compat.i386]\ndirectory=lib/i386-linux-gnu\nversion=25.08\n\n[Extension org.freedesktop.Platform.GL32]\ndirectory=lib/i386-linux-gnu/GL\nno-autodownload=true\nsubdirectories=true\nadd-ld-path=lib\nversions=25.08;1.4\ndownload-if=active-gl-driver\nenable-if=active-gl-driver\nautoprune-unless=active-gl-driver\n",
    )
    .unwrap();
    for (directory, reference) in [
        (
            "compat",
            "runtime/org.freedesktop.Platform.Compat.i386/x86_64/25.08",
        ),
        (
            "gl32",
            "runtime/org.freedesktop.Platform.GL32.default/x86_64/25.08",
        ),
    ] {
        let checkout = paths.extensions().join(directory);
        fs::create_dir_all(checkout.join("files")).unwrap();
        fs::write(
            checkout.join(".ostree-commit"),
            format!("{reference}\ncommit\n1\nflathub\n"),
        )
        .unwrap();
    }
    let app = FlatpakApp {
        app_id: "org.example.App".to_string(),
        app_dir,
        runtime_ref: "org.freedesktop.Platform/x86_64/25.08".to_string(),
        runtime_dir: PathBuf::from("/runtime"),
        command: "app".to_string(),
        args: Vec::new(),
    };

    let extensions = ensure_app_extensions(&paths, &app).unwrap();
    assert_eq!(extensions.len(), 2);
    assert!(extensions.iter().any(|extension| {
        extension.app_mount_relative == Path::new("lib/i386-linux-gnu")
            && extension.ld_library_relative.is_none()
    }));
    assert!(extensions.iter().any(|extension| {
        extension.app_mount_relative == Path::new("lib/i386-linux-gnu/GL/default")
            && extension.ld_library_relative == Some(PathBuf::from("lib"))
    }));
    let compat = extensions
        .iter()
        .find(|extension| extension.name.ends_with("Compat.i386"))
        .unwrap();
    assert!(compat.checkout_dir.join("files/GL/default").is_dir());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn missing_no_autodownload_extension_is_not_acquired() {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-no-autodownload-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let paths = Installation::for_test(&root);
    paths.ensure().unwrap();
    let app_dir = root.join("app");
    fs::create_dir_all(app_dir.join("files")).unwrap();
    fs::write(
        app_dir.join("metadata"),
        "[Application]\nname=org.example.App\nruntime=org.example.Platform/x86_64/25.08\n\n[Extension org.example.Optional]\ndirectory=lib/optional\nversion=25.08\nno-autodownload=true\n",
    )
    .unwrap();
    let app = FlatpakApp {
        app_id: "org.example.App".to_string(),
        app_dir,
        runtime_ref: "org.example.Platform/x86_64/25.08".to_string(),
        runtime_dir: PathBuf::from("/runtime"),
        command: "app".to_string(),
        args: Vec::new(),
    };
    assert!(ensure_app_extensions(&paths, &app).unwrap().is_empty());
    let _ = fs::remove_dir_all(root);
}
