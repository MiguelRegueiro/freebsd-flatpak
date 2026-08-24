use super::application_installation::InstalledApp;
use super::installation_paths::Installation;
use super::record_storage::{
    apps_dir, ensure_layout, read_kv_file, required, safe_name, write_atomic,
};
use super::runtime_records::write_runtime;
use super::{AppRecord, RuntimeRecord};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub fn record_install(paths: &Installation, installed: &InstalledApp) -> Result<AppRecord> {
    ensure_layout(paths)?;
    let app = AppRecord {
        origin: installed.origin.clone(),
        runtime_origin: installed.runtime_origin.clone(),
        app_id: installed.app_id.clone(),
        app_ref: installed.app_ref.clone(),
        app_commit: installed.app_commit.clone(),
        app_dir: paths.relative_data_path(&installed.app_dir)?,
        arch: installed.arch.clone(),
        branch: installed.branch.clone(),
        runtime_ref: installed.runtime_ref.clone(),
        runtime_commit: installed.runtime_commit.clone(),
        runtime_dir: paths.relative_data_path(&installed.runtime_dir)?,
        command: installed.command.clone(),
    };
    // The app record is the activation point used by new launches.  Publish
    // the runtime inventory first, then atomically replace the app record so a
    // reader observes either the complete old pair or the complete new pair.
    write_runtime(
        paths,
        &RuntimeRecord {
            origin: installed.runtime_origin.clone(),
            runtime_ref: app.runtime_ref.clone(),
            runtime_commit: app.runtime_commit.clone(),
            runtime_dir: app.runtime_dir.clone(),
        },
    )?;
    write_app(paths, &app)?;
    Ok(app)
}

pub fn list_apps(paths: &Installation) -> Result<Vec<AppRecord>> {
    ensure_layout(paths)?;
    let mut apps = Vec::new();
    for entry in fs::read_dir(apps_dir(paths)).context("read app state directory")? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        apps.push(read_app_path(&entry.path())?);
    }
    apps.sort_by(|left, right| left.app_id.cmp(&right.app_id));
    Ok(apps)
}

pub fn get_app(paths: &Installation, app_id: &str) -> Result<AppRecord> {
    let path = app_record_path(paths, app_id)?;
    read_app_path(&path).with_context(|| format!("{app_id} is not installed"))
}

pub fn remove_app_record(paths: &Installation, app_id: &str) -> Result<Option<AppRecord>> {
    let path = app_record_path(paths, app_id)?;
    if !path.exists() {
        return Ok(None);
    }
    let record = read_app_path(&path)?;
    fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    Ok(Some(record))
}

pub(crate) fn write_app(paths: &Installation, app: &AppRecord) -> Result<()> {
    let path = app_record_path(paths, &app.app_id)?;
    let data = format!(
        "origin={}\nruntime_origin={}\napp_id={}\napp_ref={}\napp_commit={}\napp_dir={}\narch={}\nbranch={}\nruntime_ref={}\nruntime_commit={}\nruntime_dir={}\ncommand={}\n",
        app.origin,
        app.runtime_origin,
        app.app_id,
        app.app_ref,
        app.app_commit,
        app.app_dir.display(),
        app.arch,
        app.branch,
        app.runtime_ref,
        app.runtime_commit,
        app.runtime_dir.display(),
        app.command
    );
    write_atomic(&path, data.as_bytes())
}

fn read_app_path(path: &Path) -> Result<AppRecord> {
    let values = read_kv_file(path)?;
    app_from_values(&values)
}

pub(super) fn app_from_values(
    values: &std::collections::BTreeMap<String, String>,
) -> Result<AppRecord> {
    let origin = values
        .get("origin")
        .cloned()
        .unwrap_or_else(|| crate::remotes::DEFAULT_REMOTE.to_string());
    Ok(AppRecord {
        runtime_origin: values
            .get("runtime_origin")
            .cloned()
            .unwrap_or_else(|| origin.clone()),
        origin,
        app_id: required(values, "app_id")?,
        app_ref: required(values, "app_ref")?,
        app_commit: required(values, "app_commit")?,
        app_dir: PathBuf::from(required(values, "app_dir")?),
        arch: required(values, "arch")?,
        branch: required(values, "branch")?,
        runtime_ref: required(values, "runtime_ref")?,
        runtime_commit: required(values, "runtime_commit")?,
        runtime_dir: PathBuf::from(required(values, "runtime_dir")?),
        command: required(values, "command")?,
    })
}

fn app_record_path(paths: &Installation, app_id: &str) -> Result<PathBuf> {
    Ok(apps_dir(paths).join(format!("{}.ini", safe_name(app_id)?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_records_migrate_to_flathub_origin() {
        let root = std::env::temp_dir().join(format!(
            "freebsd-flatpak-origin-migration-{}",
            std::process::id()
        ));
        let paths = Installation::for_test(&root);
        ensure_layout(&paths).unwrap();
        let record = "app_id=org.example.App\napp_ref=app/org.example.App/x86_64/stable\napp_commit=app\napp_dir=apps/app\narch=x86_64\nbranch=stable\nruntime_ref=org.example.Platform/x86_64/stable\nruntime_commit=runtime\nruntime_dir=runtimes/runtime\ncommand=example\n";
        fs::write(app_record_path(&paths, "org.example.App").unwrap(), record).unwrap();
        let migrated = get_app(&paths, "org.example.App").unwrap();
        assert_eq!(migrated.origin, crate::remotes::DEFAULT_REMOTE);
        assert_eq!(migrated.runtime_origin, crate::remotes::DEFAULT_REMOTE);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn app_and_runtime_origins_round_trip() {
        let root = std::env::temp_dir().join(format!(
            "freebsd-flatpak-origin-roundtrip-{}",
            std::process::id()
        ));
        let paths = Installation::for_test(&root);
        let app = AppRecord {
            origin: "apps".to_string(),
            runtime_origin: "runtimes".to_string(),
            app_id: "org.example.App".to_string(),
            app_ref: "app/org.example.App/x86_64/stable".to_string(),
            app_commit: "app".to_string(),
            app_dir: PathBuf::from("apps/app"),
            arch: "x86_64".to_string(),
            branch: "stable".to_string(),
            runtime_ref: "org.example.Platform/x86_64/stable".to_string(),
            runtime_commit: "runtime".to_string(),
            runtime_dir: PathBuf::from("runtimes/runtime"),
            command: "example".to_string(),
        };
        write_app(&paths, &app).unwrap();
        let loaded = get_app(&paths, &app.app_id).unwrap();
        assert_eq!(loaded.origin, "apps");
        assert_eq!(loaded.runtime_origin, "runtimes");
        let _ = fs::remove_dir_all(root);
    }
}
