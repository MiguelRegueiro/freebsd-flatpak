use super::*;
use crate::installation::application_installation::{InstallTimings, InstalledApp};
use crate::installation::{get_app, get_runtime, record_install};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

fn paths(name: &str) -> Installation {
    Installation::for_test(&std::env::temp_dir().join(format!(
        "freebsd-flatpak-size-{name}-{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    )))
}

fn installed(paths: &Installation, app_size: u64, runtime_size: u64) -> InstalledApp {
    InstalledApp {
        origin: "apps".to_string(),
        runtime_origin: "runtimes".to_string(),
        app_id: "org.example.App".to_string(),
        app_ref: "app/org.example.App/x86_64/stable".to_string(),
        app_commit: format!("app-{app_size}"),
        installed_size: app_size,
        app_dir: paths.app("org.example.App").join(format!("app-{app_size}")),
        arch: "x86_64".to_string(),
        branch: "stable".to_string(),
        runtime_ref: "org.example.Platform/x86_64/stable".to_string(),
        runtime_commit: format!("runtime-{runtime_size}"),
        runtime_installed_size: runtime_size,
        runtime_dir: paths.runtimes().join(format!("runtime-{runtime_size}")),
        command: "example".to_string(),
        timings: InstallTimings::default(),
    }
}

#[test]
fn install_and_update_persist_recalculated_app_and_runtime_sizes() {
    let paths = paths("record-update");
    record_install(&paths, &installed(&paths, 100, 200)).unwrap();
    assert_eq!(
        get_app(&paths, "org.example.App").unwrap().installed_size,
        100
    );
    assert_eq!(
        get_runtime(&paths, "org.example.Platform/x86_64/stable")
            .unwrap()
            .unwrap()
            .installed_size,
        200
    );

    record_install(&paths, &installed(&paths, 300, 400)).unwrap();
    assert_eq!(
        get_app(&paths, "org.example.App").unwrap().installed_size,
        300
    );
    assert_eq!(
        get_runtime(&paths, "org.example.Platform/x86_64/stable")
            .unwrap()
            .unwrap()
            .installed_size,
        400
    );
    let root: &Path = paths.data_root().parent().unwrap();
    fs::remove_dir_all(root).unwrap();
}
