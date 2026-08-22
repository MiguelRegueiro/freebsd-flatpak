use crate::paths::Installation;
use crate::runtime::InstalledApp;
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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

pub fn ensure_layout(paths: &Installation) -> Result<()> {
    paths.ensure()?;
    fs::create_dir_all(apps_dir(paths)).context("create app state directory")?;
    fs::create_dir_all(runtimes_dir(paths)).context("create runtime state directory")?;
    fs::create_dir_all(exports_dir(paths)).context("create export state directory")?;
    Ok(())
}

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

pub fn get_runtime(paths: &Installation, runtime_ref: &str) -> Result<Option<RuntimeRecord>> {
    let path = runtime_record_path(paths, runtime_ref);
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(read_runtime_path(&path)?))
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
    let Some((ref_name, runtime_commit)) = deployment_marker(path)? else {
        return Ok(None);
    };
    let Some(runtime_ref) = ref_name.strip_prefix("runtime/") else {
        return Ok(None);
    };
    Ok(Some(RuntimeRecord {
        runtime_ref: runtime_ref.to_string(),
        runtime_commit,
        runtime_dir: paths.relative_data_path(path)?,
    }))
}

/// Point every installed application at the current deployment of its runtime.
/// Each app record changes atomically, so concurrent launches observe a complete
/// old or new app/runtime pair.
pub fn reconcile_runtime_bindings(paths: &Installation) -> Result<()> {
    for mut app in list_apps(paths)? {
        let Some(runtime) = get_runtime(paths, &app.runtime_ref)? else {
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
    let path = runtime_record_path(paths, &runtime.runtime_ref);
    let data = format!(
        "runtime_ref={}\nruntime_commit={}\nruntime_dir={}\n",
        runtime.runtime_ref,
        runtime.runtime_commit,
        runtime.runtime_dir.display()
    );
    write_atomic(&path, data.as_bytes())
}

pub fn remove_runtime_record(paths: &Installation, runtime_ref: &str) -> Result<()> {
    let path = runtime_record_path(paths, runtime_ref);
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
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
        if crate::runtime::metadata_value(&metadata, "Application", "runtime").as_deref()
            == Some(runtime_ref)
        {
            return Ok(true);
        }
    }

    Ok(false)
}

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
        writeln!(data, "app_dir={}", app.app_dir.display())?;
        writeln!(data, "arch={}", app.arch)?;
        writeln!(data, "branch={}", app.branch)?;
        writeln!(data, "runtime_ref={}", app.runtime_ref)?;
        writeln!(data, "runtime_commit={}", app.runtime_commit)?;
        writeln!(data, "runtime_dir={}", app.runtime_dir.display())?;
        writeln!(data, "command={}", app.command)?;
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

/// Remove commit-qualified app/runtime checkouts which are neither current nor
/// pinned by a run record.  OSTree object pruning remains a separate operation.
pub fn cleanup_retired_deployments(paths: &Installation) -> Result<Vec<PathBuf>> {
    let mut protected = std::collections::BTreeSet::new();
    for app in list_apps(paths)? {
        protected.insert(absolute(paths, &app.app_dir));
        protected.insert(absolute(paths, &app.runtime_dir));
    }
    for runtime in list_runtimes(paths)? {
        protected.insert(absolute(paths, &runtime.runtime_dir));
    }
    for run in read_run_records(paths)? {
        for key in ["app_dir", "runtime_dir"] {
            if let Some(path) = run.get(key) {
                protected.insert(absolute(paths, Path::new(path)));
            }
        }
    }

    let mut removed = Vec::new();
    for root in [paths.apps(), paths.runtimes()] {
        if !root.is_dir() {
            continue;
        }
        for family in fs::read_dir(&root).with_context(|| format!("read {}", root.display()))? {
            let family = family?;
            if !family.file_type()?.is_dir() {
                continue;
            }
            let family_path = family.path();
            // Legacy checkouts live directly at `family`; generations always
            // live one level below it and are safe to identify independently.
            for generation in fs::read_dir(&family_path)
                .with_context(|| format!("read {}", family_path.display()))?
            {
                let generation = generation?;
                let path = generation.path();
                if !generation.file_type()?.is_dir()
                    || !path.join(".ostree-commit").is_file()
                    || protected.contains(&path)
                {
                    continue;
                }
                safe_remove_dir(paths, &path)?;
                removed.push(path);
            }
            // Once a legacy checkout is superseded, remove its payload while
            // retaining any commit-qualified children created beneath the
            // former checkout directory.
            if family_path.join(".ostree-commit").is_file() && !protected.contains(&family_path) {
                for entry in fs::read_dir(&family_path)
                    .with_context(|| format!("read {}", family_path.display()))?
                {
                    let entry = entry?;
                    let path = entry.path();
                    if entry.file_type()?.is_dir() && path.join(".ostree-commit").is_file() {
                        continue;
                    }
                    remove_managed_path(paths, &path)?;
                }
                removed.push(family_path);
            }
        }
    }
    Ok(removed)
}

fn remove_managed_path(paths: &Installation, path: &Path) -> Result<()> {
    let allowed = [paths.apps(), paths.runtimes(), paths.extensions()];
    if !allowed
        .iter()
        .any(|root| path.starts_with(root) && path != root)
    {
        bail!("refusing to remove unmanaged path: {}", path.display());
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            fs::remove_dir_all(path).with_context(|| format!("remove {}", path.display()))
        }
        Ok(_) => fs::remove_file(path).with_context(|| format!("remove {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

fn deployment_marker(path: &Path) -> Result<Option<(String, String)>> {
    let marker_path = path.join(".ostree-commit");
    if !marker_path.is_file() {
        return Ok(None);
    }
    let marker = fs::read_to_string(&marker_path)
        .with_context(|| format!("read {}", marker_path.display()))?;
    let mut lines = marker.lines();
    let ref_name = lines.next().context("deployment marker missing ref")?;
    let commit = lines.next().context("deployment marker missing commit")?;
    Ok(Some((ref_name.to_string(), commit.to_string())))
}

pub fn checkout_ref(path: &Path) -> Result<Option<String>> {
    Ok(deployment_marker(path)?.map(|(ref_name, _)| ref_name))
}

pub fn export_record_path(paths: &Installation, app_id: &str) -> Result<PathBuf> {
    Ok(exports_dir(paths).join(format!("{}.list", safe_name(app_id)?)))
}

pub fn write_export_record(
    installation: &Installation,
    app_id: &str,
    paths: &[PathBuf],
) -> Result<()> {
    ensure_layout(installation)?;
    let path = export_record_path(installation, app_id)?;
    let mut data = String::new();
    for path in paths {
        data.push_str(&path.display().to_string());
        data.push('\n');
    }
    write_atomic(&path, data.as_bytes())
}

pub fn read_export_record(paths: &Installation, app_id: &str) -> Result<Vec<PathBuf>> {
    let path = export_record_path(paths, app_id)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(data
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(PathBuf::from)
        .collect())
}

pub fn remove_export_record(paths: &Installation, app_id: &str) -> Result<()> {
    let path = export_record_path(paths, app_id)?;
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(())
}

pub fn absolute(paths: &Installation, path: &Path) -> PathBuf {
    paths.absolute_data_path(path)
}

pub fn safe_remove_dir(paths: &Installation, path: &Path) -> Result<()> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        paths.absolute_data_path(path)
    };
    if !path.exists() {
        return Ok(());
    }

    let allowed = [
        paths.apps(),
        paths.runtimes(),
        paths.chroots(),
        paths.extensions(),
    ];
    if !allowed
        .iter()
        .any(|root| path.starts_with(root) && path != *root)
    {
        bail!(
            "refusing to remove path outside managed runtime data: {}",
            path.display()
        );
    }

    fs::remove_dir_all(&path).with_context(|| format!("remove {}", path.display()))
}

fn write_app(paths: &Installation, app: &AppRecord) -> Result<()> {
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

fn app_from_values(values: &BTreeMap<String, String>) -> Result<AppRecord> {
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
        ".{}.{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("state"),
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
    file.write_all(data)?;
    file.flush()?;
    fs::rename(&tmp, path).with_context(|| format!("move {} to {}", tmp.display(), path.display()))
}

fn app_record_path(paths: &Installation, app_id: &str) -> Result<PathBuf> {
    Ok(apps_dir(paths).join(format!("{}.ini", safe_name(app_id)?)))
}

fn runtime_record_path(paths: &Installation, runtime_ref: &str) -> PathBuf {
    runtimes_dir(paths).join(format!("{}.ini", safe_name_lossy(runtime_ref)))
}

fn apps_dir(paths: &Installation) -> PathBuf {
    paths.refs().join("apps")
}

fn runtimes_dir(paths: &Installation) -> PathBuf {
    paths.refs().join("runtimes")
}

fn exports_dir(paths: &Installation) -> PathBuf {
    paths.refs().join("exports")
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

#[cfg(test)]
mod tests {
    use super::*;

    fn checkout(path: &Path, ref_name: &str, commit: &str) {
        fs::create_dir_all(path.join("files")).unwrap();
        fs::write(
            path.join("metadata"),
            "[Application]\nname=org.example.App\n",
        )
        .unwrap();
        fs::write(
            path.join(".ostree-commit"),
            format!("{ref_name}\n{commit}\n"),
        )
        .unwrap();
    }

    fn app(paths: &Installation, app_commit: &str, runtime_commit: &str) -> AppRecord {
        AppRecord {
            app_id: "org.example.App".to_string(),
            app_ref: "app/org.example.App/x86_64/stable".to_string(),
            app_commit: app_commit.to_string(),
            app_dir: paths
                .relative_data_path(&paths.app("org.example.App").join(app_commit))
                .unwrap(),
            arch: "x86_64".to_string(),
            branch: "stable".to_string(),
            runtime_ref: "org.example.Platform/x86_64/stable".to_string(),
            runtime_commit: runtime_commit.to_string(),
            runtime_dir: paths
                .relative_data_path(
                    &paths
                        .runtimes()
                        .join("org.example.Platform-stable")
                        .join(runtime_commit),
                )
                .unwrap(),
            command: "example".to_string(),
        }
    }

    #[test]
    fn concurrent_run_records_are_distinct_and_cleanup_is_isolated() {
        let temp = std::env::temp_dir().join(format!(
            "freebsd-flatpak-state-concurrent-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp);
        let paths = Installation::for_test(&temp);
        let first_root = paths.chroots().join("org.example.App/first");
        let second_root = paths.chroots().join("org.example.App/second");

        let first =
            write_run_record(&paths, "org.example.App", "first", &first_root, 100, 101).unwrap();
        let second =
            write_run_record(&paths, "org.example.App", "second", &second_root, 200, 201).unwrap();

        assert_ne!(first, second);
        assert_eq!(read_run_records(&paths).unwrap().len(), 2);
        remove_run_record(&first).unwrap();
        let remaining = read_run_records(&paths).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(
            remaining[0].get("instance_id").map(String::as_str),
            Some("second")
        );
        assert_eq!(
            remaining[0].get("root").map(String::as_str),
            second_root.to_str()
        );

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn multiple_pinned_generations_retire_after_their_last_run() {
        let temp = std::env::temp_dir().join(format!(
            "freebsd-flatpak-state-generations-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp);
        let paths = Installation::for_test(&temp);
        ensure_layout(&paths).unwrap();
        let persistent = paths.app_data("org.example.App").unwrap().join("sentinel");
        fs::create_dir_all(persistent.parent().unwrap()).unwrap();
        fs::write(&persistent, "keep").unwrap();

        let first = app(&paths, "app-a", "runtime-1");
        let second = app(&paths, "app-b", "runtime-2");
        let current = app(&paths, "app-c", "runtime-3");
        for deployment in [&first, &second, &current] {
            checkout(
                &absolute(&paths, &deployment.app_dir),
                &deployment.app_ref,
                &deployment.app_commit,
            );
            checkout(
                &absolute(&paths, &deployment.runtime_dir),
                &format!("runtime/{}", deployment.runtime_ref),
                &deployment.runtime_commit,
            );
        }
        write_runtime(
            &paths,
            &RuntimeRecord {
                runtime_ref: current.runtime_ref.clone(),
                runtime_commit: current.runtime_commit.clone(),
                runtime_dir: current.runtime_dir.clone(),
            },
        )
        .unwrap();
        write_app(&paths, &current).unwrap();
        let first_run = write_pinned_run_record(
            &paths,
            "first",
            &paths.chroots().join("first"),
            100,
            101,
            &first,
        )
        .unwrap();
        let second_run = write_pinned_run_record(
            &paths,
            "second",
            &paths.chroots().join("second"),
            200,
            201,
            &second,
        )
        .unwrap();

        assert!(cleanup_retired_deployments(&paths).unwrap().is_empty());
        remove_run_record(&first_run).unwrap();
        cleanup_retired_deployments(&paths).unwrap();
        assert!(!absolute(&paths, &first.app_dir).exists());
        assert!(!absolute(&paths, &first.runtime_dir).exists());
        assert!(absolute(&paths, &second.app_dir).exists());
        assert!(absolute(&paths, &second.runtime_dir).exists());

        remove_run_record(&second_run).unwrap();
        cleanup_retired_deployments(&paths).unwrap();
        assert!(!absolute(&paths, &second.app_dir).exists());
        assert!(!absolute(&paths, &second.runtime_dir).exists());
        assert!(absolute(&paths, &current.app_dir).exists());
        assert!(absolute(&paths, &current.runtime_dir).exists());
        assert_eq!(fs::read_to_string(&persistent).unwrap(), "keep");
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn recovery_reclaims_generations_after_stale_pin_is_removed() {
        let temp = std::env::temp_dir().join(format!(
            "freebsd-flatpak-state-recovery-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp);
        let paths = Installation::for_test(&temp);
        ensure_layout(&paths).unwrap();
        let retired = app(&paths, "app-old", "runtime-old");
        checkout(
            &absolute(&paths, &retired.app_dir),
            &retired.app_ref,
            &retired.app_commit,
        );
        checkout(
            &absolute(&paths, &retired.runtime_dir),
            &format!("runtime/{}", retired.runtime_ref),
            &retired.runtime_commit,
        );
        let run = write_pinned_run_record(
            &paths,
            "crashed",
            &paths.chroots().join("crashed"),
            i32::MAX as u32,
            0,
            &retired,
        )
        .unwrap();
        assert!(cleanup_retired_deployments(&paths).unwrap().is_empty());
        remove_run_record(&run).unwrap();
        assert_eq!(cleanup_retired_deployments(&paths).unwrap().len(), 2);
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn shared_runtime_activation_updates_every_future_launch_record() {
        let temp = std::env::temp_dir().join(format!(
            "freebsd-flatpak-state-shared-runtime-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp);
        let paths = Installation::for_test(&temp);
        ensure_layout(&paths).unwrap();
        let mut first = app(&paths, "app-one", "runtime-1");
        let mut second = app(&paths, "app-two", "runtime-1");
        second.app_id = "org.example.Other".to_string();
        second.app_ref = "app/org.example.Other/x86_64/stable".to_string();
        second.app_dir = paths
            .relative_data_path(&paths.app("org.example.Other").join("app-two"))
            .unwrap();
        write_app(&paths, &first).unwrap();
        write_app(&paths, &second).unwrap();
        let new_runtime_dir = paths
            .runtimes()
            .join("org.example.Platform-stable/runtime-2");
        write_runtime(
            &paths,
            &RuntimeRecord {
                runtime_ref: first.runtime_ref.clone(),
                runtime_commit: "runtime-2".to_string(),
                runtime_dir: paths.relative_data_path(&new_runtime_dir).unwrap(),
            },
        )
        .unwrap();

        reconcile_runtime_bindings(&paths).unwrap();
        first = get_app(&paths, &first.app_id).unwrap();
        second = get_app(&paths, &second.app_id).unwrap();
        assert_eq!(first.runtime_commit, "runtime-2");
        assert_eq!(second.runtime_commit, "runtime-2");
        assert_eq!(absolute(&paths, &first.runtime_dir), new_runtime_dir);
        assert_eq!(first.runtime_dir, second.runtime_dir);
        let _ = fs::remove_dir_all(&temp);
    }
}
