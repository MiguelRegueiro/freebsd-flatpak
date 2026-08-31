use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

fn fixture() -> (Installation, PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-local-runtime-extension-{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    let paths = Installation::for_test(&root);
    fs::create_dir_all(paths.remote_configs()).unwrap();
    fs::write(paths.remote_configs().join("poison.conf"), [0xff]).unwrap();
    let runtime_dir = root.join("runtime");
    fs::create_dir_all(runtime_dir.join("files/lib/GL/default")).unwrap();
    fs::create_dir_all(runtime_dir.join("files/lib/dri/intel")).unwrap();
    fs::write(
        runtime_dir.join("metadata"),
        "[Runtime]\nname=org.example.Platform\n\
         [Extension org.gtk.Gtk3theme]\nversion=3.22\nversions=9.99;3.22\ndirectory=share/runtime/share/themes\nsubdirectory-suffix=gtk-3.0\n\
         [Extension org.freedesktop.Platform.GL]\nversions=25.08;24.08\ndirectory=lib/GL\n\
         [Extension org.freedesktop.Platform.VAAPI.Intel]\nversion=24.08\ndirectory=lib/dri/intel\nadd-ld-path=lib\n",
    )
    .unwrap();
    (paths, runtime_dir, root)
}

fn install_extension(paths: &Installation, name: &str, branch: &str, origin: &str) -> PathBuf {
    let checkout = paths.extensions().join(format!("{name}-{branch}"));
    fs::create_dir_all(checkout.join("files")).unwrap();
    fs::write(
        checkout.join("metadata"),
        format!("[Runtime]\nname={name}\n"),
    )
    .unwrap();
    fs::write(
        checkout.join(".ostree-commit"),
        format!("runtime/{name}/x86_64/{branch}\ncommit-{branch}\n42\n{origin}\n"),
    )
    .unwrap();
    checkout
}

#[test]
fn valid_gl_and_vaapi_activate_from_local_deployments_with_selected_branches() {
    let (paths, runtime_dir, root) = fixture();
    let gl_checkout = install_extension(
        &paths,
        "org.freedesktop.Platform.GL.default",
        "25.08",
        "runtime-origin",
    );
    let vaapi_checkout = install_extension(
        &paths,
        "org.freedesktop.Platform.VAAPI.Intel",
        "24.08",
        "fallback-origin",
    );

    let gl =
        activate_default_gl_extension(&paths, "org.example.Platform/x86_64/24.08", &runtime_dir)
            .unwrap()
            .unwrap();
    let vaapi =
        activate_intel_vaapi_extension(&paths, "org.example.Platform/x86_64/24.08", &runtime_dir)
            .unwrap()
            .unwrap();

    assert_eq!(
        gl.ref_name,
        "runtime/org.freedesktop.Platform.GL.default/x86_64/25.08"
    );
    assert_eq!(gl.checkout_dir, gl_checkout);
    assert_eq!(
        vaapi.ref_name,
        "runtime/org.freedesktop.Platform.VAAPI.Intel/x86_64/24.08"
    );
    assert_eq!(vaapi.checkout_dir, vaapi_checkout);
    assert_eq!(vaapi.ld_library_relative.as_deref(), Some(Path::new("lib")));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn active_gtk_theme_extension_prepares_runtime_declared_target() {
    let (paths, runtime_dir, root) = fixture();
    let checkout = install_extension(
        &paths,
        "org.gtk.Gtk3theme.Example-Dark",
        "3.22",
        "runtime-origin",
    );

    let extension = activate_gtk_theme_extension(
        &paths,
        "org.example.Platform/x86_64/24.08",
        &runtime_dir,
        Some("Example-Dark"),
    )
    .unwrap()
    .unwrap();

    assert_eq!(extension.checkout_dir, checkout);
    assert_eq!(
        extension.runtime_mount_relative,
        Path::new("share/runtime/share/themes/Example-Dark/gtk-3.0")
    );
    assert!(runtime_dir
        .join("files/share/runtime/share/themes/Example-Dark/gtk-3.0")
        .is_dir());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn gtk_theme_uses_first_declared_versions_branch_when_version_is_absent() {
    let (paths, runtime_dir, root) = fixture();
    fs::write(
        runtime_dir.join("metadata"),
        "[Runtime]\nname=org.example.Platform\n\
         [Extension org.gtk.Gtk3theme]\nversions=; 3.24;3.22;\ndirectory=share/runtime/share/themes\nsubdirectory-suffix=gtk-3.0\n",
    )
    .unwrap();
    let checkout = install_extension(
        &paths,
        "org.gtk.Gtk3theme.Example-Dark",
        "3.24",
        "runtime-origin",
    );

    let extension = activate_gtk_theme_extension(
        &paths,
        "org.example.Platform/x86_64/24.08",
        &runtime_dir,
        Some("Example-Dark"),
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        extension.ref_name,
        "runtime/org.gtk.Gtk3theme.Example-Dark/x86_64/3.24"
    );
    assert_eq!(extension.checkout_dir, checkout);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unavailable_or_corrupt_gtk_theme_extension_is_skipped() {
    let (paths, runtime_dir, root) = fixture();

    let missing = activate_gtk_theme_extension(
        &paths,
        "org.example.Platform/x86_64/24.08",
        &runtime_dir,
        Some("Missing"),
    )
    .unwrap();
    assert!(missing.is_none());

    let checkout = install_extension(
        &paths,
        "org.gtk.Gtk3theme.Example-Dark",
        "3.22",
        "runtime-origin",
    );
    fs::write(checkout.join(".ostree-commit"), "wrong-ref\n").unwrap();
    let corrupt = activate_gtk_theme_extension(
        &paths,
        "org.example.Platform/x86_64/24.08",
        &runtime_dir,
        Some("Example-Dark"),
    )
    .unwrap();
    assert!(corrupt.is_none());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn runtime_codec_activates_at_declared_directory_with_declared_version() {
    let (paths, runtime_dir, root) = fixture();
    let mount_relative = Path::new("lib/x86_64-linux-gnu/codecs-extra");
    fs::create_dir_all(runtime_dir.join("files").join(mount_relative)).unwrap();
    fs::write(
        runtime_dir.join("metadata"),
        "[Runtime]\nname=org.example.Platform\n\
         [Extension org.freedesktop.Platform.codecs-extra]\ndirectory=lib/x86_64-linux-gnu/codecs-extra\nversion=25.08-extra\nadd-ld-path=lib\n",
    )
    .unwrap();
    let checkout = install_extension(
        &paths,
        "org.freedesktop.Platform.codecs-extra",
        "25.08-extra",
        "runtime-origin",
    );

    let extensions =
        activate_runtime_codec_extensions(&paths, "org.example.Platform/x86_64/50", &runtime_dir)
            .unwrap();

    assert_eq!(extensions.len(), 1);
    let extension = &extensions[0];
    assert_eq!(
        extension.ref_name,
        "runtime/org.freedesktop.Platform.codecs-extra/x86_64/25.08-extra"
    );
    assert_eq!(extension.checkout_dir, checkout);
    assert_eq!(extension.runtime_mount_relative, mount_relative);
    assert_eq!(
        extension.ld_library_relative.as_deref(),
        Some(Path::new("lib"))
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn missing_or_corrupt_local_extension_has_actionable_error() {
    let (paths, runtime_dir, root) = fixture();
    let missing =
        activate_default_gl_extension(&paths, "org.example.Platform/x86_64/24.08", &runtime_dir)
            .unwrap_err();
    let message = format!("{missing:#}");
    assert!(message.contains("missing or corrupt"));
    assert!(message.contains("flatpak update"));
    assert!(message.contains("flatpak repair"));

    let checkout = install_extension(
        &paths,
        "org.freedesktop.Platform.GL.default",
        "25.08",
        "runtime-origin",
    );
    fs::write(checkout.join(".ostree-commit"), "wrong-ref\n").unwrap();
    let corrupt =
        activate_default_gl_extension(&paths, "org.example.Platform/x86_64/24.08", &runtime_dir)
            .unwrap_err();
    let message = format!("{corrupt:#}");
    assert!(message.contains("missing or corrupt"));
    assert!(message.contains("expected runtime/org.freedesktop.Platform.GL.default/x86_64/25.08"));
    assert!(message.contains("flatpak repair"));
    fs::remove_dir_all(root).unwrap();
}
