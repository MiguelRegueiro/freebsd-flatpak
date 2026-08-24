use super::application_records::{list_apps, write_app};
use super::generation_cleanup::deployment_data;
use super::installation_paths::Installation;
use super::record_storage::{
    ensure_layout, read_kv_file, required, runtimes_dir, safe_name_lossy, write_atomic,
};
use super::RuntimeRecord;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub fn get_runtime(paths: &Installation, runtime_ref: &str) -> Result<Option<RuntimeRecord>> {
    Ok(list_runtimes(paths)?
        .into_iter()
        .find(|record| record.runtime_ref == runtime_ref))
}

pub fn get_runtime_from(
    paths: &Installation,
    origin: &str,
    runtime_ref: &str,
) -> Result<Option<RuntimeRecord>> {
    Ok(list_runtimes(paths)?
        .into_iter()
        .find(|record| record.origin == origin && record.runtime_ref == runtime_ref))
}

pub fn list_runtimes(paths: &Installation) -> Result<Vec<RuntimeRecord>> {
    ensure_layout(paths)?;
    let mut runtimes = Vec::new();
    for entry in fs::read_dir(runtimes_dir(paths)).context("read runtime state directory")? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            runtimes.push(read_runtime_path(&entry.path())?);
        }
    }
    runtimes.sort_by(|left, right| left.runtime_ref.cmp(&right.runtime_ref));
    Ok(runtimes)
}

pub fn list_runtime_deployments(paths: &Installation) -> Result<Vec<RuntimeRecord>> {
    ensure_layout(paths)?;
    let mut deployments = Vec::new();
    let root = paths.runtimes();
    if !root.is_dir() {
        return Ok(deployments);
    }
    for family in fs::read_dir(&root).with_context(|| format!("read {}", root.display()))? {
        let family = family?;
        if !family.file_type()?.is_dir() {
            continue;
        }
        if let Some(runtime) = runtime_deployment_from_path(paths, &family.path())? {
            deployments.push(runtime);
        }
        for generation in fs::read_dir(family.path())? {
            let generation = generation?;
            if !generation.file_type()?.is_dir() {
                continue;
            }
            if let Some(runtime) = runtime_deployment_from_path(paths, &generation.path())? {
                deployments.push(runtime);
            }
        }
    }
    deployments.sort_by(|left, right| {
        left.runtime_ref
            .cmp(&right.runtime_ref)
            .then_with(|| left.runtime_dir.cmp(&right.runtime_dir))
    });
    deployments.dedup_by(|left, right| left.runtime_dir == right.runtime_dir);
    Ok(deployments)
}

fn runtime_deployment_from_path(
    paths: &Installation,
    path: &Path,
) -> Result<Option<RuntimeRecord>> {
    let Some(data) = deployment_data(path)? else {
        return Ok(None);
    };
    let Some(runtime_ref) = data.ref_name.strip_prefix("runtime/") else {
        return Ok(None);
    };
    Ok(Some(RuntimeRecord {
        origin: data.origin,
        runtime_ref: runtime_ref.to_string(),
        runtime_commit: data.commit,
        installed_size: data.installed_size,
        runtime_dir: paths.relative_data_path(path)?,
    }))
}

/// Point every installed application at the current deployment of its runtime.
/// Each app record changes atomically, so concurrent launches observe a complete
/// old or new app/runtime pair.
pub fn reconcile_runtime_bindings(paths: &Installation) -> Result<()> {
    for mut app in list_apps(paths)? {
        let Some(runtime) = get_runtime_from(paths, &app.runtime_origin, &app.runtime_ref)? else {
            continue;
        };
        if app.runtime_commit == runtime.runtime_commit && app.runtime_dir == runtime.runtime_dir {
            continue;
        }
        app.runtime_commit = runtime.runtime_commit;
        app.runtime_dir = runtime.runtime_dir;
        write_app(paths, &app)?;
    }
    Ok(())
}

pub fn write_runtime(paths: &Installation, runtime: &RuntimeRecord) -> Result<()> {
    ensure_layout(paths)?;
    let path = existing_runtime_record_path(paths, &runtime.origin, &runtime.runtime_ref)?
        .unwrap_or_else(|| runtime_record_path(paths, &runtime.origin, &runtime.runtime_ref));
    let data = format!(
        "origin={}\nruntime_ref={}\nruntime_commit={}\ninstalled_size={}\nruntime_dir={}\n",
        runtime.origin,
        runtime.runtime_ref,
        runtime.runtime_commit,
        runtime.installed_size,
        runtime.runtime_dir.display()
    );
    write_atomic(&path, data.as_bytes())
}

pub fn remove_runtime_record(paths: &Installation, runtime_ref: &str) -> Result<()> {
    for entry in fs::read_dir(runtimes_dir(paths)).context("read runtime state directory")? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && read_runtime_path(&entry.path())?.runtime_ref == runtime_ref
        {
            fs::remove_file(entry.path())
                .with_context(|| format!("remove runtime record {runtime_ref}"))?;
        }
    }
    Ok(())
}

pub fn runtime_is_required(paths: &Installation, runtime_ref: &str) -> Result<bool> {
    for app in list_apps(paths)? {
        if app.runtime_ref == runtime_ref {
            return Ok(true);
        }
    }

    let app_root = paths.apps();
    if !app_root.is_dir() {
        return Ok(false);
    }

    for entry in fs::read_dir(&app_root).with_context(|| format!("read {}", app_root.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let metadata_path = entry.path().join("metadata");
        let Ok(metadata) = fs::read_to_string(&metadata_path) else {
            continue;
        };
        if crate::installation::metadata_value(&metadata, "Application", "runtime").as_deref()
            == Some(runtime_ref)
        {
            return Ok(true);
        }
    }

    Ok(false)
}

pub(super) fn read_runtime_path(path: &Path) -> Result<RuntimeRecord> {
    let values = read_kv_file(path)?;
    Ok(RuntimeRecord {
        origin: values
            .get("origin")
            .cloned()
            .unwrap_or_else(|| crate::remotes::DEFAULT_REMOTE.to_string()),
        runtime_ref: required(&values, "runtime_ref")?,
        runtime_commit: required(&values, "runtime_commit")?,
        installed_size: required(&values, "installed_size")?
            .parse()
            .context("invalid installed_size")?,
        runtime_dir: PathBuf::from(required(&values, "runtime_dir")?),
    })
}

fn runtime_record_path(paths: &Installation, origin: &str, runtime_ref: &str) -> PathBuf {
    runtimes_dir(paths).join(format!(
        "{}--{}.ini",
        safe_name_lossy(origin),
        safe_name_lossy(runtime_ref)
    ))
}

fn existing_runtime_record_path(
    paths: &Installation,
    origin: &str,
    runtime_ref: &str,
) -> Result<Option<PathBuf>> {
    for entry in fs::read_dir(runtimes_dir(paths)).context("read runtime state directory")? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            let record = read_runtime_path(&entry.path())?;
            if record.origin == origin && record.runtime_ref == runtime_ref {
                return Ok(Some(entry.path()));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
#[path = "tests/runtime_records.rs"]
mod tests;
