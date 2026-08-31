use super::required_extension_refs;
use std::collections::BTreeSet;
use std::fs;

#[test]
fn active_gl_default_subextension_is_required_by_runtime_metadata() {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-runtime-gl-reachability-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let app = root.join("app");
    let runtime = root.join("runtime");
    fs::create_dir_all(&app).unwrap();
    fs::create_dir_all(&runtime).unwrap();
    fs::write(
        app.join("metadata"),
        "[Application]\nname=org.example.App\n",
    )
    .unwrap();
    fs::write(
            runtime.join("metadata"),
            "[Runtime]\nname=org.freedesktop.Platform\n\n[Extension org.freedesktop.Platform.GL]\ndirectory=lib/x86_64-linux-gnu/GL\nversions=25.08;25.08-extra;1.4\nsubdirectories=true\ndownload-if=active-gl-driver\nenable-if=active-gl-driver\nautoprune-unless=active-gl-driver\n",
        )
        .unwrap();
    let gl_default = "runtime/org.freedesktop.Platform.GL.default/x86_64/25.08".to_string();
    let installed = BTreeSet::from([
        gl_default.clone(),
        "runtime/org.freedesktop.Platform.GL.default/x86_64/24.08".to_string(),
        "runtime/org.freedesktop.Platform.GL.vendor/x86_64/25.08".to_string(),
    ]);

    let required = required_extension_refs(
        &app,
        "org.freedesktop.Platform/x86_64/25.08",
        &runtime,
        &installed,
        None,
    )
    .unwrap();
    assert_eq!(required, BTreeSet::from([gl_default]));
    let _ = fs::remove_dir_all(&root);
}

fn gtk_theme_fixture(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-gtk-reachability-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let app = root.join("app");
    let runtime = root.join("runtime");
    fs::create_dir_all(&app).unwrap();
    fs::create_dir_all(&runtime).unwrap();
    fs::write(
        app.join("metadata"),
        "[Application]\nname=org.example.App\n",
    )
    .unwrap();
    fs::write(
        runtime.join("metadata"),
        "[Runtime]\nname=org.example.Platform\n\n[Extension org.gtk.Gtk3theme]\ndirectory=share/runtime/share/themes\nversion=3.22\nsubdirectories=true\nsubdirectory-suffix=gtk-3.0\ndownload-if=active-gtk-theme\nenable-if=active-gtk-theme\n",
    )
    .unwrap();
    (app, runtime)
}

#[test]
fn active_gtk_theme_extension_is_required() {
    let (app, runtime) = gtk_theme_fixture("active");
    let active = "runtime/org.gtk.Gtk3theme.Adwaita/x86_64/3.22".to_string();
    let installed = BTreeSet::from([active.clone()]);

    let required = required_extension_refs(
        &app,
        "org.example.Platform/x86_64/50",
        &runtime,
        &installed,
        Some("Adwaita"),
    )
    .unwrap();

    assert_eq!(required, BTreeSet::from([active]));
    let _ = fs::remove_dir_all(app.parent().unwrap());
}

#[test]
fn previously_active_gtk_theme_becomes_unused_after_switch() {
    let (app, runtime) = gtk_theme_fixture("switched");
    let previous = "runtime/org.gtk.Gtk3theme.Adwaita/x86_64/3.22".to_string();
    let current = "runtime/org.gtk.Gtk3theme.Breeze/x86_64/3.22".to_string();
    let installed = BTreeSet::from([previous, current.clone()]);

    let required = required_extension_refs(
        &app,
        "org.example.Platform/x86_64/50",
        &runtime,
        &installed,
        Some("Breeze"),
    )
    .unwrap();

    assert_eq!(required, BTreeSet::from([current]));
    let _ = fs::remove_dir_all(app.parent().unwrap());
}

#[test]
fn unrelated_gtk_theme_namespace_matches_are_not_required() {
    let (app, runtime) = gtk_theme_fixture("unrelated");
    let active = "runtime/org.gtk.Gtk3theme.Breeze/x86_64/3.22".to_string();
    let installed = BTreeSet::from([
        active.clone(),
        "runtime/org.gtk.Gtk3theme.Adwaita/x86_64/3.22".to_string(),
        "runtime/org.gtk.Gtk3theme.Unrelated/x86_64/3.22".to_string(),
    ]);

    let required = required_extension_refs(
        &app,
        "org.example.Platform/x86_64/50",
        &runtime,
        &installed,
        Some("Breeze"),
    )
    .unwrap();

    assert_eq!(required, BTreeSet::from([active]));
    let _ = fs::remove_dir_all(app.parent().unwrap());
}
