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
    )
    .unwrap();
    assert_eq!(required, BTreeSet::from([gl_default]));
    let _ = fs::remove_dir_all(&root);
}
