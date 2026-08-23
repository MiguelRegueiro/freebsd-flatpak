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

pub(super) fn write_app(paths: &Installation, app: &AppRecord) -> Result<()> {
    let path = app_record_path(paths, &app.app_id)?;
    let data = format!(
        "app_id={}\napp_ref={}\napp_commit={}\napp_dir={}\narch={}\nbranch={}\nruntime_ref={}\nruntime_commit={}\nruntime_dir={}\ncommand={}\n",
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
    Ok(AppRecord {
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
