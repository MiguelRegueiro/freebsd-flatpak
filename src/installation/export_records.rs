use super::installation_paths::Installation;
use super::record_storage::{ensure_layout, exports_dir, safe_name, write_atomic};
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

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
