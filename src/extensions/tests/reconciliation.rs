use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

fn fixture() -> (Installation, AppRecord, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-extension-reconciliation-{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    let paths = Installation::for_test(&root);
    let app_dir = paths.app("org.example.App").join("app-commit");
    let runtime_dir = paths.runtimes().join("runtime-commit");
    fs::create_dir_all(app_dir.join("files")).unwrap();
    fs::create_dir_all(runtime_dir.join("files")).unwrap();
    fs::write(
        runtime_dir.join("metadata"),
        "[Runtime]\nname=org.example.Platform\n\
         [Extension org.gtk.Gtk3theme]\nversion=3.22\ndirectory=share/runtime/share/themes\nsubdirectories=true\nsubdirectory-suffix=gtk-3.0\n\
         [Extension org.freedesktop.Platform.GL]\nversions=24.08;23.08;\nversion=1.4\ndirectory=lib/GL\nsubdirectories=true\ndownload-if=active-gl-driver\n\
         [Extension org.freedesktop.Platform.VAAPI.Intel]\nversion=24.08\ndirectory=lib/dri/intel\n\
         [Extension org.freedesktop.Platform.codecs-extra]\ndirectory=lib/codecs\nversion=24.08-extra\nadd-ld-path=lib\n",
    )
    .unwrap();
    fs::write(
        app_dir.join("metadata"),
        "[Application]\nname=org.example.App\nruntime=org.example.Platform/x86_64/24.08\n\
         [Extension org.freedesktop.Platform.ffmpeg-full]\ndirectory=lib/ffmpeg\n\
         [Extension org.example.Plugin]\ndirectory=lib/plugins\nversion=stable\n",
    )
    .unwrap();
    let record = AppRecord {
        origin: "app-remote".to_string(),
        runtime_origin: "runtime-remote".to_string(),
        app_id: "org.example.App".to_string(),
        app_ref: "app/org.example.App/x86_64/stable".to_string(),
        app_commit: "app-commit".to_string(),
        installed_size: 0,
        app_dir: paths.relative_data_path(&app_dir).unwrap(),
        arch: "x86_64".to_string(),
        branch: "stable".to_string(),
        runtime_ref: "org.example.Platform/x86_64/24.08".to_string(),
        runtime_commit: "runtime-commit".to_string(),
        runtime_dir: paths.relative_data_path(&runtime_dir).unwrap(),
        command: "example".to_string(),
    };
    (paths, record, root)
}

fn available() -> BTreeSet<String> {
    [
        "runtime/org.gtk.Gtk3theme.Example-Dark/x86_64/3.22",
        "runtime/org.freedesktop.Platform.GL.default/x86_64/24.08",
        "runtime/org.freedesktop.Platform.GL.default/x86_64/23.08",
        "runtime/org.freedesktop.Platform.VAAPI.Intel/x86_64/24.08",
        "runtime/org.freedesktop.Platform.codecs-extra/x86_64/24.08-extra",
        "runtime/org.freedesktop.Platform.ffmpeg-full/x86_64/stable",
        "runtime/org.example.Plugin/x86_64/stable",
    ]
    .into_iter()
    .map(ToOwned::to_owned)
    .collect()
}

#[test]
fn generic_discovery_preserves_gl_vaapi_theme_codec_and_app_extensions() {
    let (paths, app, root) = fixture();
    let required =
        required_for_app(&paths, &app, &available(), true, Some("Example-Dark")).unwrap();
    let refs = required
        .iter()
        .map(|extension| extension.ref_name.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        refs,
        BTreeSet::from([
            "runtime/org.example.Plugin/x86_64/stable",
            "runtime/org.freedesktop.Platform.GL.default/x86_64/24.08",
            "runtime/org.freedesktop.Platform.VAAPI.Intel/x86_64/24.08",
            "runtime/org.freedesktop.Platform.codecs-extra/x86_64/24.08-extra",
            "runtime/org.freedesktop.Platform.ffmpeg-full/x86_64/stable",
            "runtime/org.gtk.Gtk3theme.Example-Dark/x86_64/3.22",
        ])
    );
    assert!(crate::installation::absolute(&paths, &app.runtime_dir)
        .join("files/lib/GL/default")
        .is_dir());
    assert!(crate::installation::absolute(&paths, &app.runtime_dir)
        .join("files/share/runtime/share/themes/Example-Dark/gtk-3.0")
        .is_dir());
    assert!(crate::installation::absolute(&paths, &app.app_dir)
        .join("files/lib/plugins")
        .is_dir());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn host_specific_selection_does_not_regress_gl_or_vaapi_policy() {
    let (paths, app, root) = fixture();
    let required = required_for_app(&paths, &app, &available(), false, None).unwrap();
    let refs = required
        .iter()
        .map(|extension| extension.ref_name.as_str())
        .collect::<BTreeSet<_>>();
    assert!(refs.contains("runtime/org.freedesktop.Platform.GL.default/x86_64/24.08"));
    assert!(!refs.contains("runtime/org.freedesktop.Platform.VAAPI.Intel/x86_64/24.08"));
    assert!(!refs.contains("runtime/org.gtk.Gtk3theme.Example-Dark/x86_64/3.22"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extension_payload_destination_uses_normal_runtime_identity_and_tree() {
    let (paths, _app, root) = fixture();
    let (destination, explicit) = runtime_destination(
        &paths,
        "runtime/org.example.Plugin/x86_64/stable",
        "flathub",
        "plugin-commit",
        None,
        false,
    );
    assert_eq!(
        destination,
        paths
            .runtimes()
            .join("org.example.Plugin-x86_64-stable/plugin-commit")
    );
    assert!(destination.starts_with(paths.runtimes()));
    assert!(!explicit);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn directly_installed_runtime_deployment_is_reused_without_duplication() {
    let (paths, _app, root) = fixture();
    crate::installation::ensure_layout(&paths).unwrap();
    let checkout = paths
        .runtimes()
        .join("org.example.Plugin-x86_64-stable/plugin-commit");
    fs::create_dir_all(checkout.join("files")).unwrap();
    let record = RuntimeRecord {
        origin: "flathub".to_string(),
        runtime_ref: "org.example.Plugin/x86_64/stable".to_string(),
        runtime_commit: "plugin-commit".to_string(),
        installed_size: 42,
        explicitly_installed: true,
        runtime_dir: paths.relative_data_path(&checkout).unwrap(),
    };
    crate::installation::write_runtime(&paths, &record).unwrap();

    let (destination, explicit) = runtime_destination(
        &paths,
        "runtime/org.example.Plugin/x86_64/stable",
        "flathub",
        "plugin-commit",
        Some(&record),
        false,
    );
    assert_eq!(destination, checkout);
    assert!(explicit);
    let installed = crate::installation::list_runtimes(&paths).unwrap();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].runtime_ref, record.runtime_ref);
    fs::remove_dir_all(root).unwrap();
}
