use super::application_records::{app_from_values, get_app};
use super::generation_cleanup::deployment_marker;
use super::installation_paths::Installation;
use super::record_storage::{ensure_layout, read_kv_file, required, safe_name, write_atomic};
use super::AppRecord;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub fn run_record_path(paths: &Installation, app_id: &str, instance_id: &str) -> Result<PathBuf> {
    Ok(paths.runs().join(format!(
        "{}.{}.ini",
        safe_name(app_id)?,
        safe_name(instance_id)?
    )))
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn write_run_record(
    paths: &Installation,
    app_id: &str,
    instance_id: &str,
    root: &Path,
    launcher_pid: u32,
    child_pid: u32,
) -> Result<PathBuf> {
    write_run_record_inner(
        paths,
        app_id,
        instance_id,
        root,
        launcher_pid,
        child_pid,
        None,
        None,
        &[],
    )
}

pub fn write_checkout_pin(
    paths: &Installation,
    app_id: &str,
    instance_id: &str,
    root: &Path,
    app_dir: &Path,
    runtime_dir: &Path,
) -> Result<PathBuf> {
    write_run_record_inner(
        paths,
        app_id,
        instance_id,
        root,
        std::process::id(),
        0,
        None,
        Some((app_dir, runtime_dir)),
        &[],
    )
}

pub fn write_pinned_run_record(
    paths: &Installation,
    instance_id: &str,
    root: &Path,
    launcher_pid: u32,
    child_pid: u32,
    app: &AppRecord,
) -> Result<PathBuf> {
    write_pinned_run_record_with_extensions(
        paths,
        instance_id,
        root,
        launcher_pid,
        child_pid,
        app,
        &[],
    )
}

pub fn write_pinned_run_record_with_extensions(
    paths: &Installation,
    instance_id: &str,
    root: &Path,
    launcher_pid: u32,
    child_pid: u32,
    app: &AppRecord,
    extension_refs: &[String],
) -> Result<PathBuf> {
    write_run_record_inner(
        paths,
        &app.app_id,
        instance_id,
        root,
        launcher_pid,
        child_pid,
        Some(app),
        None,
        extension_refs,
    )
}

#[allow(clippy::too_many_arguments)]
fn write_run_record_inner(
    paths: &Installation,
    app_id: &str,
    instance_id: &str,
    root: &Path,
    launcher_pid: u32,
    child_pid: u32,
    deployment: Option<&AppRecord>,
    checkout_paths: Option<(&Path, &Path)>,
    extension_refs: &[String],
) -> Result<PathBuf> {
    ensure_layout(paths)?;
    let path = run_record_path(paths, app_id, instance_id)?;
    let mut data = format!(
        "app_id={app_id}\ninstance_id={instance_id}\nroot={}\nlauncher_pid={launcher_pid}\nchild_pid={child_pid}\n",
        root.display()
    );
    if let Some(app) = deployment {
        use std::fmt::Write as _;
        writeln!(data, "app_ref={}", app.app_ref)?;
        writeln!(data, "app_commit={}", app.app_commit)?;
        writeln!(data, "installed_size={}", app.installed_size)?;
        writeln!(data, "app_dir={}", app.app_dir.display())?;
        writeln!(data, "arch={}", app.arch)?;
        writeln!(data, "branch={}", app.branch)?;
        writeln!(data, "runtime_ref={}", app.runtime_ref)?;
        writeln!(data, "runtime_commit={}", app.runtime_commit)?;
        writeln!(data, "runtime_dir={}", app.runtime_dir.display())?;
        writeln!(data, "command={}", app.command)?;
    } else if let Some((app_dir, runtime_dir)) = checkout_paths {
        use std::fmt::Write as _;
        writeln!(data, "app_dir={}", app_dir.display())?;
        writeln!(data, "runtime_dir={}", runtime_dir.display())?;
    }
    if !extension_refs.is_empty() {
        use std::fmt::Write as _;
        writeln!(data, "extension_refs={}", extension_refs.join(";"))?;
    }
    write_atomic(&path, data.as_bytes())?;
    Ok(path)
}

pub fn app_from_run_record(
    paths: &Installation,
    values: &BTreeMap<String, String>,
) -> Result<AppRecord> {
    if values.contains_key("app_commit") {
        return app_from_values(values);
    }
    get_app(paths, &required(values, "app_id")?)
}

pub fn pinned_deployment_for_app(
    paths: &Installation,
    app_id: &str,
    app_dir: &Path,
    runtime_dir: &Path,
) -> Result<AppRecord> {
    let mut app = get_app(paths, app_id)?;
    app.app_dir = paths
        .relative_data_path(app_dir)
        .unwrap_or_else(|_| app_dir.to_path_buf());
    app.runtime_dir = paths
        .relative_data_path(runtime_dir)
        .unwrap_or_else(|_| runtime_dir.to_path_buf());

    if let Some((ref_name, commit)) = deployment_marker(app_dir)? {
        app.app_ref = ref_name;
        app.app_commit = commit;
    }
    if let Some((ref_name, commit)) = deployment_marker(runtime_dir)? {
        app.runtime_ref = ref_name
            .strip_prefix("runtime/")
            .unwrap_or(&ref_name)
            .to_string();
        app.runtime_commit = commit;
    }
    Ok(app)
}

pub fn remove_run_record(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(())
}

pub fn mark_run_record_portal_inactive(path: &Path) -> Result<()> {
    let data = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut updated = String::with_capacity(data.len() + 24);
    for line in data.lines() {
        if !line.starts_with("portal_active=") {
            updated.push_str(line);
            updated.push('\n');
        }
    }
    updated.push_str("portal_active=false\n");
    write_atomic(path, updated.as_bytes())
        .with_context(|| format!("mark portal inactive in {}", path.display()))
}

pub fn read_run_records(paths: &Installation) -> Result<Vec<BTreeMap<String, String>>> {
    ensure_layout(paths)?;
    let mut records = Vec::new();
    for entry in fs::read_dir(paths.runs()).context("read run state directory")? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let mut record = read_kv_file(&entry.path())?;
        record.insert("_path".to_string(), entry.path().display().to_string());
        records.push(record);
    }
    Ok(records)
}

#[cfg(test)]
#[path = "tests/run_records.rs"]
mod tests;
