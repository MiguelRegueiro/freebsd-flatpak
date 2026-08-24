use super::installation_paths::Installation;
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn ensure_layout(paths: &Installation) -> Result<()> {
    paths.ensure()?;
    fs::create_dir_all(apps_dir(paths)).context("create app state directory")?;
    fs::create_dir_all(runtimes_dir(paths)).context("create runtime state directory")?;
    fs::create_dir_all(exports_dir(paths)).context("create export state directory")?;
    Ok(())
}

pub(super) fn read_kv_file(path: &Path) -> Result<BTreeMap<String, String>> {
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

pub(super) fn required(values: &BTreeMap<String, String>, key: &str) -> Result<String> {
    values
        .get(key)
        .cloned()
        .with_context(|| format!("state record missing {key}"))
}

pub(crate) fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
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

pub(super) fn apps_dir(paths: &Installation) -> PathBuf {
    paths.refs().join("apps")
}

pub(super) fn runtimes_dir(paths: &Installation) -> PathBuf {
    paths.refs().join("runtimes")
}

pub(super) fn exports_dir(paths: &Installation) -> PathBuf {
    paths.refs().join("exports")
}

pub(super) fn safe_name(value: &str) -> Result<String> {
    if value.contains('/') {
        bail!("name must not contain '/': {value}");
    }
    Ok(safe_name_lossy(value))
}

pub(super) fn safe_name_lossy(value: &str) -> String {
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
