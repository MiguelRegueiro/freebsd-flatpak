use crate::installation as state;
use crate::installation::installation_paths::Installation;
use crate::remotes;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static TEST_ID: AtomicUsize = AtomicUsize::new(0);

pub(super) fn test_dir(name: &str) -> PathBuf {
    let id = TEST_ID.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "freebsd-flatpak-poc-main-{name}-{}-{id}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

pub(super) fn create_checkout(root: &Path, rel: &Path) {
    let dir = root.join(rel);
    fs::create_dir_all(dir.join("files")).unwrap();
    fs::write(
        dir.join("metadata"),
        "[Application]\nname=org.example.App\n",
    )
    .unwrap();
}

pub(super) fn app_record(app_id: &str, app_ref: &str, app_commit: &str) -> state::AppRecord {
    state::AppRecord {
        app_id: app_id.to_string(),
        app_ref: app_ref.to_string(),
        app_commit: app_commit.to_string(),
        app_dir: PathBuf::from("apps").join(app_id),
        arch: "x86_64".to_string(),
        branch: "stable".to_string(),
        runtime_ref: "org.example.Platform/x86_64/stable".to_string(),
        runtime_commit: "runtime-1".to_string(),
        runtime_dir: PathBuf::from("runtimes").join("org.example.Platform-stable"),
        command: "old-command".to_string(),
    }
}

pub(super) fn remote_app(app_id: &str, app_ref: &str, app_commit: &str) -> remotes::RemoteApp {
    remotes::RemoteApp {
        app_id: app_id.to_string(),
        app_ref: app_ref.to_string(),
        app_commit: app_commit.to_string(),
        arch: "x86_64".to_string(),
        branch: "stable".to_string(),
        runtime_ref: "org.example.Platform/x86_64/stable".to_string(),
        runtime_commit: "runtime-1".to_string(),
        sdk_ref: None,
        download_size: None,
        installed_size: None,
        command: "new-command".to_string(),
    }
}

pub(super) fn create_runtime_checkout(paths: &Installation) {
    create_checkout(
        paths.data_root(),
        &PathBuf::from("runtimes").join("org.example.Platform-stable"),
    );
}

pub(super) fn create_marked_checkout(path: &Path, ref_name: &str, commit: &str, metadata: &str) {
    fs::create_dir_all(path.join("files")).unwrap();
    fs::write(path.join("metadata"), metadata).unwrap();
    fs::write(
        path.join(".ostree-commit"),
        format!("{ref_name}\n{commit}\n"),
    )
    .unwrap();
}
