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
         [Extension org.gtk.Gtk3theme]\nversion=3.22\ndirectory=share/runtime/share/themes\nsubdirectory-suffix=gtk-3.0\n\
         [Extension org.freedesktop.Platform.GL]\nversions=24.08;23.08;\nversion=1.4\ndirectory=lib/GL\n\
         [Extension org.freedesktop.Platform.VAAPI.Intel]\nversion=24.08\ndirectory=lib/dri/intel\n",
    )
    .unwrap();
    fs::write(
        app_dir.join("metadata"),
        "[Application]\nname=org.example.App\nruntime=org.example.Platform/x86_64/24.08\n\
         [Extension org.freedesktop.Platform.ffmpeg-full]\nversion=24.08\ndirectory=lib/ffmpeg\n\
         [Extension org.example.Unsupported]\nversion=24.08\ndirectory=lib/unsupported\n",
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

#[test]
fn discovers_gl_and_supported_app_extensions_without_vaapi_on_non_intel_hosts() {
    let (paths, app, root) = fixture();

    let required = required_for_app(&paths, &app, false, None).unwrap();
    let refs = required
        .iter()
        .map(|extension| extension.ref_name.as_str())
        .collect::<BTreeSet<_>>();

    assert_eq!(
        refs,
        BTreeSet::from([
            "runtime/org.freedesktop.Platform.GL.default/x86_64/24.08",
            "runtime/org.freedesktop.Platform.ffmpeg-full/x86_64/24.08",
        ])
    );
    assert_eq!(required[0].preferred_origin, "runtime-remote");
    assert_eq!(required[1].preferred_origin, "app-remote");
    assert!(crate::installation::absolute(&paths, &app.runtime_dir)
        .join("files/lib/GL/default")
        .is_dir());
    assert!(crate::installation::absolute(&paths, &app.app_dir)
        .join("files/lib/ffmpeg")
        .is_dir());
    assert!(!crate::installation::absolute(&paths, &app.app_dir)
        .join("files/lib/unsupported")
        .exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn discovers_runtime_codec_extension_with_its_declared_branch_and_mountpoint() {
    let (paths, app, root) = fixture();
    let runtime_dir = crate::installation::absolute(&paths, &app.runtime_dir);
    fs::write(
        runtime_dir.join("metadata"),
        "[Runtime]\nname=org.example.Platform\n\
         [Extension org.freedesktop.Platform.codecs-extra]\ndirectory=lib/x86_64-linux-gnu/codecs-extra\nversion=25.08-extra\nadd-ld-path=lib\n",
    )
    .unwrap();

    let required = required_for_app(&paths, &app, false, None).unwrap();
    let codec = required
        .iter()
        .find(|extension| extension.ref_name.contains("codecs-extra"))
        .unwrap();

    assert_eq!(
        codec.ref_name,
        "runtime/org.freedesktop.Platform.codecs-extra/x86_64/25.08-extra"
    );
    assert_eq!(codec.preferred_origin, "runtime-remote");
    assert_eq!(
        codec.checkout_dir,
        paths
            .extensions()
            .join("org.freedesktop.Platform.codecs-extra-25.08-extra")
    );
    assert!(runtime_dir
        .join("files/lib/x86_64-linux-gnu/codecs-extra")
        .is_dir());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn active_gtk_theme_adds_optional_runtime_extension() {
    let (paths, app, root) = fixture();

    let required = required_for_app(&paths, &app, false, Some("Example-Dark")).unwrap();
    let theme = required
        .iter()
        .find(|extension| extension.ref_name.contains("org.gtk.Gtk3theme"))
        .unwrap();

    assert_eq!(
        theme.ref_name,
        "runtime/org.gtk.Gtk3theme.Example-Dark/x86_64/3.22"
    );
    assert!(theme.optional);
    assert!(!crate::installation::absolute(&paths, &app.runtime_dir)
        .join("files/share/runtime/share/themes/Example-Dark/gtk-3.0")
        .exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn adds_hardware_dependent_vaapi_when_intel_is_present() {
    let (paths, app, root) = fixture();

    let required = required_for_app(&paths, &app, true, None).unwrap();

    assert!(required.iter().any(|extension| {
        extension.ref_name == "runtime/org.freedesktop.Platform.VAAPI.Intel/x86_64/24.08"
            && extension.preferred_origin == "runtime-remote"
    }));
    assert!(crate::installation::absolute(&paths, &app.runtime_dir)
        .join("files/lib/dri/intel")
        .is_dir());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn discovers_extensions_from_the_activated_runtime_deployment() {
    let (paths, app, root) = fixture();
    let current_runtime = paths.runtimes().join("runtime-current");
    fs::create_dir_all(current_runtime.join("files")).unwrap();
    fs::write(
        current_runtime.join("metadata"),
        "[Runtime]\nname=org.example.Platform\n\
         [Extension org.freedesktop.Platform.GL]\nversions=25.08;1.4\nversion=1.4\ndirectory=lib/current-gl\n",
    )
    .unwrap();
    crate::installation::write_runtime(
        &paths,
        &crate::installation::RuntimeRecord {
            origin: app.runtime_origin.clone(),
            runtime_ref: app.runtime_ref.clone(),
            runtime_commit: "runtime-current".to_string(),
            installed_size: 0,
            runtime_dir: paths.relative_data_path(&current_runtime).unwrap(),
        },
    )
    .unwrap();

    let required = required_for_app(&paths, &app, false, None).unwrap();

    assert!(required.iter().any(|extension| {
        extension.ref_name == "runtime/org.freedesktop.Platform.GL.default/x86_64/25.08"
    }));
    assert!(current_runtime
        .join("files/lib/current-gl/default")
        .is_dir());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn corrupt_deployment_markers_do_not_block_origin_fallback() {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-extension-origin-{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join(".ostree-commit"),
        "runtime/org.example.Extension/x86_64/stable\ncommit\n123\norigin\n",
    )
    .unwrap();
    assert_eq!(
        deployment_origin(&root, "runtime/org.example.Extension/x86_64/stable").as_deref(),
        Some("origin")
    );

    fs::write(root.join(".ostree-commit"), "corrupt\n").unwrap();
    assert_eq!(
        deployment_origin(&root, "runtime/org.example.Extension/x86_64/stable"),
        None
    );

    fs::remove_dir_all(root).unwrap();
}
