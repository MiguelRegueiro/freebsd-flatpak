use super::*;
use crate::installation::{
    self as state, installation_paths::Installation, FlatpakApp, RuntimeRecord,
};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

fn fixture() -> (Installation, FlatpakApp, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "freebsd-flatpak-activation-{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    let paths = Installation::for_test(&root);
    state::ensure_layout(&paths).unwrap();
    let app_dir = paths.app("org.example.App").join("app-commit");
    let runtime_dir = paths.runtimes().join("platform").join("runtime-commit");
    fs::create_dir_all(app_dir.join("files")).unwrap();
    fs::create_dir_all(runtime_dir.join("files/share/icons")).unwrap();
    fs::write(runtime_dir.join("files/share/icons/base"), "base").unwrap();
    fs::write(
        runtime_dir.join("metadata"),
        "[Runtime]\nname=org.example.Platform\n\
         [Extension org.example.Graphics]\ndirectory=lib/extensions\nversion=1\nsubdirectories=true\nsubdirectory-suffix=payload\nadd-ld-path=lib:extra\nenable-if=active-gl-driver;on-xdg-desktop-KDE\nmerge-dirs=share/icons\n\
         [Extension org.example.Parent]\ndirectory=lib/parent\nversion=1\n\
         [Extension org.example.Child]\ndirectory=lib/parent/child\nversion=1\n\
         [Extension org.example.Disabled]\ndirectory=lib/disabled\nversion=1\nenable-if=have-intel-gpu\n",
    )
    .unwrap();
    fs::write(
        app_dir.join("metadata"),
        "[Application]\nname=org.example.App\nruntime=org.example.Platform/x86_64/1\ncommand=example\n\
         [Extension org.example.AppPlugin]\ndirectory=extensions/plugin\nversion=1\nadd-ld-path=lib\n",
    )
    .unwrap();
    for (name, priority) in [
        ("org.example.Graphics.default", 10),
        ("org.example.Parent", 0),
        ("org.example.Child", 0),
        ("org.example.Graphics.low", 1),
        ("org.example.Disabled", 0),
        ("org.example.AppPlugin", 0),
    ] {
        let partial = format!("{name}/x86_64/1");
        let full = format!("runtime/{partial}");
        let checkout = paths
            .runtimes()
            .join(super::super::runtime_extensions::runtime_checkout_dir(
                &partial,
            ))
            .join(format!("commit-{priority}"));
        let merge_dir = checkout.join("files/share/icons");
        fs::create_dir_all(&merge_dir).unwrap();
        if name == "org.example.Graphics.default" {
            fs::write(merge_dir.join("conflict"), "winner").unwrap();
        } else if name == "org.example.Graphics.low" {
            fs::write(merge_dir.join("conflict"), "loser").unwrap();
        }
        fs::write(
            checkout.join("metadata"),
            format!("[Runtime]\nname={name}\n[ExtensionOf]\npriority={priority}\n"),
        )
        .unwrap();
        fs::write(
            checkout.join(".ostree-commit"),
            format!("{full}\ncommit-{priority}\n1\nflathub\n"),
        )
        .unwrap();
        state::write_runtime(
            &paths,
            &RuntimeRecord {
                origin: "flathub".to_string(),
                runtime_ref: partial,
                runtime_commit: format!("commit-{priority}"),
                installed_size: 1,
                explicitly_installed: false,
                runtime_dir: paths.relative_data_path(&checkout).unwrap(),
            },
        )
        .unwrap();
    }
    (
        paths,
        FlatpakApp {
            app_id: "org.example.App".to_string(),
            app_dir,
            runtime_ref: "org.example.Platform/x86_64/1".to_string(),
            runtime_dir,
            command: "example".to_string(),
            args: Vec::new(),
        },
        root,
    )
}

fn gl_mount(name: &str, target: &str) -> ExtensionMount {
    ExtensionMount {
        name: name.to_string(),
        ref_name: format!("runtime/{name}/x86_64/25.08"),
        commit: format!("{name}-commit"),
        checkout_dir: PathBuf::from("/extensions").join(name),
        target: PathBuf::from(target),
        add_ld_paths: Vec::new(),
        merge_dirs: Vec::new(),
        priority: 0,
        scope: ExtensionScope::Runtime,
        conditions: vec!["active-gl-driver".to_string()],
    }
}

#[test]
fn conditions_are_or_expressions_and_match_dynamic_suffixes() {
    let facts = ExtensionFacts {
        active_gl_drivers: BTreeSet::from(["default".to_string(), "low".to_string()]),
        active_gtk_theme: Some("Adwaita-dark".to_string()),
        intel_gpu: false,
        kernel_modules: BTreeSet::from(["nvidia_drm".to_string()]),
        xdg_desktops: BTreeSet::from(["gnome".to_string()]),
    };
    assert!(facts.matches_any(
        &["have-intel-gpu".into(), "on-xdg-desktop-GNOME".into()],
        "org.example.Extension"
    ));
    assert!(facts.matches_any(&["active-gl-driver".into()], "org.example.Graphics.default"));
    assert!(facts.matches_any(
        &["active-gtk-theme".into()],
        "org.example.Theme.Adwaita-dark"
    ));
    assert!(facts.matches_any(
        &["have-kernel-module-nvidia-drm".into()],
        "org.example.Driver"
    ));
    assert!(!facts.matches_any(
        &["have-intel-gpu".into(), "on-xdg-desktop-KDE".into()],
        "org.example.Extension"
    ));
}

#[test]
fn freedesktop_debug_symbols_are_mounted_but_never_the_gl_provider() {
    let debug = gl_mount(
        "org.freedesktop.Platform.GL.Debug.default",
        "usr/lib/debug/usr/lib/x86_64-linux-gnu/GL/default",
    );
    let normal = gl_mount(
        "org.freedesktop.Platform.GL.default",
        "usr/lib/x86_64-linux-gnu/GL/default",
    );
    let plan = ExtensionMountPlan {
        mounts: vec![debug.clone(), normal.clone()],
    };

    assert!(plan.mounts.contains(&debug));
    assert_eq!(plan.active_gl_mount(), Some(&normal));
    assert_eq!(
        ExtensionMountPlan {
            mounts: vec![debug],
        }
        .active_gl_mount(),
        None
    );
}

#[test]
fn kde_broad_gl_point_cannot_promote_debug_symbols_to_provider() {
    let debug = gl_mount(
        "org.freedesktop.Platform.GL.Debug.default",
        "usr/lib/x86_64-linux-gnu/GL/Debug.default",
    );
    let normal = gl_mount(
        "org.freedesktop.Platform.GL.default",
        "usr/lib/x86_64-linux-gnu/GL/default",
    );
    let plan = ExtensionMountPlan {
        // KDE mount ordering puts the upper-case Debug directory first.
        mounts: vec![debug.clone(), normal.clone()],
    };

    assert_eq!(plan.conditioned_mount("active-gl-driver"), Some(&debug));
    assert_eq!(plan.active_gl_mount(), Some(&normal));
}

#[test]
fn arbitrary_ids_resolve_to_one_ordered_mount_plan_without_mutating_parents() {
    let (paths, app, root) = fixture();
    let facts = ExtensionFacts {
        active_gl_drivers: BTreeSet::from(["default".to_string()]),
        ..ExtensionFacts::default()
    };
    let absent_target = app.runtime_dir.join("files/lib/extensions/default/payload");
    assert!(!absent_target.exists());

    let plan = resolve_extension_mount_plan(&paths, &app, &facts).unwrap();
    let refs = plan.refs();
    assert!(refs.contains(&"runtime/org.example.Graphics.default/x86_64/1".to_string()));
    assert!(refs.contains(&"runtime/org.example.AppPlugin/x86_64/1".to_string()));
    assert!(!refs.contains(&"runtime/org.example.Disabled/x86_64/1".to_string()));
    assert_eq!(
        plan.conditioned_mount("active-gl-driver").unwrap().target,
        PathBuf::from("usr/lib/extensions/default/payload")
    );
    assert!(plan
        .runtime_ld_library_paths()
        .contains(&"/usr/lib/extensions/default/payload/lib".to_string()));
    assert!(plan
        .app_ld_library_paths()
        .contains(&"/app/extensions/plugin/lib".to_string()));
    let parent = plan
        .mounts
        .iter()
        .position(|mount| mount.name == "org.example.Parent")
        .unwrap();
    let child = plan
        .mounts
        .iter()
        .position(|mount| mount.name == "org.example.Child")
        .unwrap();
    assert!(parent < child);
    assert!(!absent_target.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn merge_entries_use_extensionof_priority_and_stable_ref_tiebreaking() {
    let (paths, app, root) = fixture();
    let facts = ExtensionFacts {
        active_gl_drivers: BTreeSet::from(["default".to_string(), "low".to_string()]),
        ..ExtensionFacts::default()
    };
    let plan = resolve_extension_mount_plan(&paths, &app, &facts).unwrap();
    let merges = plan
        .merge_directories(&app.app_dir, &app.runtime_dir)
        .unwrap();
    assert_eq!(merges.len(), 1);
    assert_eq!(
        merges[0].target,
        PathBuf::from("usr/lib/extensions/share/icons")
    );
    assert_eq!(merges[0].entries.len(), 1);
    assert_eq!(merges[0].entries[0].name, PathBuf::from("conflict"));
    assert_eq!(
        fs::read_to_string(&merges[0].entries[0].source).unwrap(),
        "winner"
    );
    fs::remove_dir_all(root).unwrap();
}
