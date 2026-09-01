use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

fn fixture(metadata: &str) -> (Installation, FlatpakApp, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-local-app-extension-{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    let paths = Installation::for_test(&root);
    let app_dir = root.join("app");
    let runtime_dir = root.join("runtime");
    fs::create_dir_all(app_dir.join("files")).unwrap();
    fs::create_dir_all(runtime_dir.join("files")).unwrap();
    fs::write(app_dir.join("metadata"), metadata).unwrap();
    let app = FlatpakApp {
        app_id: "org.example.App".to_string(),
        app_dir,
        runtime_ref: "org.example.Platform/x86_64/24.08".to_string(),
        runtime_dir,
        command: "example".to_string(),
        args: Vec::new(),
    };
    (paths, app, root)
}

#[test]
fn valid_app_extension_activates_from_normal_runtime_record() {
    let (paths, app, root) = fixture(
        "[Application]\nname=org.example.App\n\
         [Extension org.freedesktop.Platform.ffmpeg-full]\nversions=23.08;22.08\ndirectory=lib/ffmpeg\nadd-ld-path=lib\n",
    );
    fs::create_dir_all(app.app_dir.join("files/lib/ffmpeg")).unwrap();
    crate::installation::ensure_layout(&paths).unwrap();
    let runtime_ref = "org.freedesktop.Platform.ffmpeg-full/x86_64/23.08";
    let checkout = paths
        .runtimes()
        .join(crate::installation::runtime_checkout_dir(runtime_ref))
        .join("commit");
    fs::create_dir_all(checkout.join("files")).unwrap();
    fs::write(
        checkout.join("metadata"),
        "[Runtime]\nname=org.freedesktop.Platform.ffmpeg-full\n",
    )
    .unwrap();
    fs::write(
        checkout.join(".ostree-commit"),
        "runtime/org.freedesktop.Platform.ffmpeg-full/x86_64/23.08\ncommit\n7\napp-origin\n",
    )
    .unwrap();
    crate::installation::write_runtime(
        &paths,
        &crate::installation::RuntimeRecord {
            origin: "app-origin".to_string(),
            runtime_ref: runtime_ref.to_string(),
            runtime_commit: "commit".to_string(),
            installed_size: 7,
            explicitly_installed: false,
            runtime_dir: paths.relative_data_path(&checkout).unwrap(),
        },
    )
    .unwrap();

    let extensions = activate_app_codec_extensions(&paths, &app).unwrap();

    assert_eq!(extensions.len(), 1);
    assert_eq!(
        extensions[0].ref_name,
        "runtime/org.freedesktop.Platform.ffmpeg-full/x86_64/23.08"
    );
    assert_eq!(extensions[0].checkout_dir, checkout);
    assert_eq!(
        extensions[0].ld_library_relative.as_deref(),
        Some(std::path::Path::new("lib"))
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn app_without_launch_compatibility_consumer_activates_no_codec_mount() {
    let (paths, app, root) = fixture(
        "[Application]\nname=org.example.App\n\
         [Extension org.example.Optional]\nversion=24.08\ndirectory=lib/optional\n",
    );

    let extensions = activate_app_codec_extensions(&paths, &app).unwrap();

    assert!(extensions.is_empty());
    fs::remove_dir_all(root).unwrap();
}
