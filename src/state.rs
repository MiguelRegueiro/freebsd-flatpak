use crate::runtime::InstalledApp;
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AppRecord {
    pub app_id: String,
    pub app_ref: String,
    pub app_commit: String,
    pub app_dir: PathBuf,
    pub arch: String,
    pub branch: String,
    pub runtime_ref: String,
    pub runtime_commit: String,
    pub runtime_dir: PathBuf,
    pub command: String,
}

#[derive(Debug, Clone)]
pub struct RuntimeRecord {
    pub runtime_ref: String,
    pub runtime_commit: String,
    pub runtime_dir: PathBuf,
}

pub fn ensure_layout(project_root: &Path) -> Result<()> {
    fs::create_dir_all(apps_dir(project_root)).context("create app state directory")?;
    fs::create_dir_all(runtimes_dir(project_root)).context("create runtime state directory")?;
    fs::create_dir_all(runs_dir(project_root)).context("create run state directory")?;
    Ok(())
}

pub fn record_install(project_root: &Path, installed: &InstalledApp) -> Result<AppRecord> {
    ensure_layout(project_root)?;
    let app = AppRecord {
        app_id: installed.app_id.clone(),
        app_ref: installed.app_ref.clone(),
        app_commit: installed.app_commit.clone(),
        app_dir: relative_to(project_root, &installed.app_dir)?,
        arch: installed.arch.clone(),
        branch: installed.branch.clone(),
        runtime_ref: installed.runtime_ref.clone(),
        runtime_commit: installed.runtime_commit.clone(),
        runtime_dir: relative_to(project_root, &installed.runtime_dir)?,
        command: installed.command.clone(),
    };
    write_app(project_root, &app)?;
    write_runtime(
        project_root,
        &RuntimeRecord {
            runtime_ref: app.runtime_ref.clone(),
            runtime_commit: app.runtime_commit.clone(),
            runtime_dir: app.runtime_dir.clone(),
        },
    )?;
    Ok(app)
}

pub fn list_apps(project_root: &Path) -> Result<Vec<AppRecord>> {
    ensure_layout(project_root)?;
    let mut apps = Vec::new();
    for entry in fs::read_dir(apps_dir(project_root)).context("read app state directory")? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        apps.push(read_app_path(&entry.path())?);
    }
    apps.sort_by(|left, right| left.app_id.cmp(&right.app_id));
    Ok(apps)
}

pub fn get_app(project_root: &Path, app_id: &str) -> Result<AppRecord> {
    let path = app_record_path(project_root, app_id)?;
    read_app_path(&path).with_context(|| format!("{app_id} is not installed"))
}

pub fn remove_app_record(project_root: &Path, app_id: &str) -> Result<Option<AppRecord>> {
    let path = app_record_path(project_root, app_id)?;
    if !path.exists() {
        return Ok(None);
    }
    let record = read_app_path(&path)?;
    fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    Ok(Some(record))
}

pub fn runtime_commit(project_root: &Path, runtime_ref: &str) -> Result<Option<String>> {
    let path = runtime_record_path(project_root, runtime_ref);
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(read_runtime_path(&path)?.runtime_commit))
}

pub fn write_runtime(project_root: &Path, runtime: &RuntimeRecord) -> Result<()> {
    ensure_layout(project_root)?;
    let path = runtime_record_path(project_root, &runtime.runtime_ref);
    let data = format!(
        "runtime_ref={}\nruntime_commit={}\nruntime_dir={}\n",
        runtime.runtime_ref,
        runtime.runtime_commit,
        runtime.runtime_dir.display()
    );
    write_atomic(&path, data.as_bytes())
}

pub fn remove_runtime_record(project_root: &Path, runtime_ref: &str) -> Result<()> {
    let path = runtime_record_path(project_root, runtime_ref);
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(())
}

pub fn runtime_is_required(project_root: &Path, runtime_ref: &str) -> Result<bool> {
    for app in list_apps(project_root)? {
        if app.runtime_ref == runtime_ref {
            return Ok(true);
        }
    }

    let app_root = project_root.join("runtime").join("app");
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
        if crate::runtime::metadata_value(&metadata, "Application", "runtime").as_deref()
            == Some(runtime_ref)
        {
            return Ok(true);
        }
    }

    Ok(false)
}

pub fn run_record_path(project_root: &Path, app_id: &str) -> Result<PathBuf> {
    Ok(runs_dir(project_root).join(format!("{}.ini", safe_name(app_id)?)))
}

pub fn write_run_record(
    project_root: &Path,
    app_id: &str,
    root: &Path,
    launcher_pid: u32,
    child_pid: u32,
) -> Result<PathBuf> {
    ensure_layout(project_root)?;
    let path = run_record_path(project_root, app_id)?;
    let root = relative_to(project_root, root)?;
    let data = format!(
        "app_id={app_id}\nroot={}\nlauncher_pid={launcher_pid}\nchild_pid={child_pid}\n",
        root.display()
    );
    write_atomic(&path, data.as_bytes())?;
    Ok(path)
}

pub fn remove_run_record(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(())
}

pub fn read_run_records(project_root: &Path) -> Result<Vec<BTreeMap<String, String>>> {
    ensure_layout(project_root)?;
    let mut records = Vec::new();
    for entry in fs::read_dir(runs_dir(project_root)).context("read run state directory")? {
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

pub fn absolute(project_root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    }
}

pub fn safe_remove_dir(project_root: &Path, path: &Path) -> Result<()> {
    let path = absolute(project_root, path);
    if !path.exists() {
        return Ok(());
    }

    let allowed = [
        project_root.join("runtime").join("app"),
        project_root.join("runtime").join("chroots"),
        project_root.join("runtime"),
    ];
    if !allowed.iter().any(|root| path.starts_with(root)) || path == project_root.join("runtime") {
        bail!(
            "refusing to remove path outside managed runtime data: {}",
            path.display()
        );
    }

    fs::remove_dir_all(&path).with_context(|| format!("remove {}", path.display()))
}

fn write_app(project_root: &Path, app: &AppRecord) -> Result<()> {
    let path = app_record_path(project_root, &app.app_id)?;
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
    Ok(AppRecord {
        app_id: required(&values, "app_id")?,
        app_ref: required(&values, "app_ref")?,
        app_commit: required(&values, "app_commit")?,
        app_dir: PathBuf::from(required(&values, "app_dir")?),
        arch: required(&values, "arch")?,
        branch: required(&values, "branch")?,
        runtime_ref: required(&values, "runtime_ref")?,
        runtime_commit: required(&values, "runtime_commit")?,
        runtime_dir: PathBuf::from(required(&values, "runtime_dir")?),
        command: required(&values, "command")?,
    })
}

fn read_runtime_path(path: &Path) -> Result<RuntimeRecord> {
    let values = read_kv_file(path)?;
    Ok(RuntimeRecord {
        runtime_ref: required(&values, "runtime_ref")?,
        runtime_commit: required(&values, "runtime_commit")?,
        runtime_dir: PathBuf::from(required(&values, "runtime_dir")?),
    })
}

fn read_kv_file(path: &Path) -> Result<BTreeMap<String, String>> {
    let data = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut values = BTreeMap::new();
    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            bail!("invalid state line in {}: {line}", path.display());
        };
        values.insert(key.to_string(), value.to_string());
    }
    Ok(values)
}

fn required(values: &BTreeMap<String, String>, key: &str) -> Result<String> {
    values
        .get(key)
        .cloned()
        .with_context(|| format!("state record missing {key}"))
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("state"),
        std::process::id()
    ));
    let mut file = fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
    file.write_all(data)?;
    file.flush()?;
    fs::rename(&tmp, path).with_context(|| format!("move {} to {}", tmp.display(), path.display()))
}

fn app_record_path(project_root: &Path, app_id: &str) -> Result<PathBuf> {
    Ok(apps_dir(project_root).join(format!("{}.ini", safe_name(app_id)?)))
}

fn runtime_record_path(project_root: &Path, runtime_ref: &str) -> PathBuf {
    runtimes_dir(project_root).join(format!("{}.ini", safe_name_lossy(runtime_ref)))
}

fn apps_dir(project_root: &Path) -> PathBuf {
    project_root.join("state").join("apps")
}

fn runtimes_dir(project_root: &Path) -> PathBuf {
    project_root.join("state").join("runtimes")
}

fn runs_dir(project_root: &Path) -> PathBuf {
    project_root.join("state").join("runs")
}

fn relative_to(project_root: &Path, path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    };
    Ok(absolute
        .strip_prefix(project_root)
        .with_context(|| {
            format!(
                "{} is outside {}",
                absolute.display(),
                project_root.display()
            )
        })?
        .to_path_buf())
}

fn safe_name(value: &str) -> Result<String> {
    if value.contains('/') {
        bail!("name must not contain '/': {value}");
    }
    Ok(safe_name_lossy(value))
}

fn safe_name_lossy(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}
