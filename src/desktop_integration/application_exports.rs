use super::desktop_caches::refresh_export_caches;
use super::desktop_entries::rewrite_desktop_file;
use super::xdg_data_projection::{
    cleanup_managed_projections_for_app, publish_projection, remove_projection, ProjectionOutcome,
};
use crate::installation::installation_paths::Installation;
use crate::installation::{self as state, AppRecord};
use anyhow::{bail, Context, Result};
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs as unix_fs;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Default, Clone)]
pub struct ExportReport {
    pub files: usize,
    pub desktop_entries: usize,
    pub skipped: Vec<PathBuf>,
    pub conflicts: Vec<PathBuf>,
}

pub fn export_data_dir(paths: &Installation) -> PathBuf {
    paths.export_share()
}

pub fn export_app(paths: &Installation, app: &AppRecord) -> Result<ExportReport> {
    cleanup_managed_projections_for_app(paths, &app.app_id)?;
    remove_export_files(paths, &app.app_id)?;

    let source_share = state::absolute(paths, &app.app_dir)
        .join("export")
        .join("share");
    let mut report = ExportReport::default();
    if !source_share.is_dir() {
        eprintln!(
            "warning: {} has no exported desktop data at {}",
            app.app_id,
            source_share.display()
        );
        return Ok(report);
    }

    let export_share = export_data_dir(paths);
    fs::create_dir_all(&export_share)
        .with_context(|| format!("create export data dir {}", export_share.display()))?;

    let flatpak_bin = paths.launcher();
    if !flatpak_bin.is_file() {
        eprintln!(
            "warning: exported desktop entries will call {}, but it does not exist yet",
            flatpak_bin.display()
        );
    }

    let mut exported_paths = Vec::new();
    copy_export_dir(
        &source_share,
        &source_share,
        &export_share,
        flatpak_bin,
        &app.app_id,
        &mut exported_paths,
        &mut report,
    )?;
    exported_paths.sort();
    for rel in &exported_paths {
        if publish_projection(paths, rel)? == ProjectionOutcome::PreservedConflict {
            report.conflicts.push(rel.clone());
        }
    }
    state::write_export_record(paths, &app.app_id, &exported_paths)?;
    refresh_export_caches(paths)?;

    report.files = exported_paths.len();
    Ok(report)
}

pub fn remove_export(paths: &Installation, app_id: &str) -> Result<()> {
    remove_export_files(paths, app_id)?;
    cleanup_managed_projections_for_app(paths, app_id)?;
    refresh_export_caches(paths)
}

fn remove_export_files(paths: &Installation, app_id: &str) -> Result<()> {
    let export_share = export_data_dir(paths);
    let mut parents = Vec::new();

    for rel in state::read_export_record(paths, app_id)? {
        validate_relative_export_path(&rel)?;
        let target = export_share.join(&rel);
        remove_projection(paths, &rel, &target)?;
        if let Some(parent) = target.parent() {
            parents.push(parent.to_path_buf());
        }

        let Ok(metadata) = fs::symlink_metadata(&target) else {
            continue;
        };
        if metadata.file_type().is_dir() {
            bail!(
                "refusing to remove directory listed as exported file: {}",
                target.display()
            );
        }
        fs::remove_file(&target).with_context(|| format!("remove {}", target.display()))?;
    }

    parents.sort();
    parents.dedup();
    for parent in parents.into_iter().rev() {
        remove_empty_parents(&export_share, &parent)?;
    }

    state::remove_export_record(paths, app_id)?;
    Ok(())
}

fn copy_export_dir(
    source_root: &Path,
    source_dir: &Path,
    export_share: &Path,
    flatpak_bin: &Path,
    app_id: &str,
    exported_paths: &mut Vec<PathBuf>,
    report: &mut ExportReport,
) -> Result<()> {
    let mut entries = fs::read_dir(source_dir)
        .with_context(|| format!("read export directory {}", source_dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let source = entry.path();
        let rel = source
            .strip_prefix(source_root)
            .with_context(|| format!("make {} relative", source.display()))?
            .to_path_buf();
        validate_relative_export_path(&rel)?;

        if should_skip_export_path(&rel) {
            if rel.components().count() == 1 {
                report.skipped.push(rel);
            }
            continue;
        }

        let target = export_share.join(&rel);
        let metadata =
            fs::symlink_metadata(&source).with_context(|| format!("stat {}", source.display()))?;
        let file_type = metadata.file_type();

        if file_type.is_dir() {
            fs::create_dir_all(&target)
                .with_context(|| format!("create export dir {}", target.display()))?;
            copy_export_dir(
                source_root,
                &source,
                export_share,
                flatpak_bin,
                app_id,
                exported_paths,
                report,
            )?;
            continue;
        }

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create export dir {}", parent.display()))?;
        }
        remove_existing_export_file(&target)?;

        if file_type.is_symlink() {
            let link_target = fs::read_link(&source)
                .with_context(|| format!("read link {}", source.display()))?;
            unix_fs::symlink(&link_target, &target)
                .with_context(|| format!("symlink {}", target.display()))?;
        } else if file_type.is_file() {
            if is_desktop_file(&rel) {
                rewrite_desktop_file(&source, &target, flatpak_bin, app_id)?;
                report.desktop_entries += 1;
            } else {
                fs::copy(&source, &target).with_context(|| {
                    format!("copy {} to {}", source.display(), target.display())
                })?;
            }
            fs::set_permissions(&target, metadata.permissions())
                .with_context(|| format!("set permissions on {}", target.display()))?;
        } else {
            eprintln!(
                "warning: skipping unsupported export file type {}",
                source.display()
            );
            continue;
        }

        exported_paths.push(rel);
    }

    Ok(())
}

fn should_skip_export_path(rel: &Path) -> bool {
    matches!(
        rel.components().next(),
        Some(Component::Normal(name)) if name == "dbus-1" || name == "gnome-shell"
    )
}

fn is_desktop_file(rel: &Path) -> bool {
    rel.extension().and_then(|ext| ext.to_str()) == Some("desktop")
}

pub(super) fn validate_relative_export_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        bail!("invalid export path: {}", path.display());
    }
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir => {}
            _ => bail!("invalid export path: {}", path.display()),
        }
    }
    Ok(())
}

fn remove_existing_export_file(path: &Path) -> Result<()> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if metadata.file_type().is_dir() {
        bail!("refusing to replace export directory {}", path.display());
    }
    fs::remove_file(path).with_context(|| format!("remove old export {}", path.display()))
}

pub(super) fn remove_empty_parents(root: &Path, leaf: &Path) -> Result<()> {
    let mut current = leaf.to_path_buf();
    while current.starts_with(root) && current != root {
        match fs::remove_dir(&current) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) if error.kind() == ErrorKind::DirectoryNotEmpty => break,
            Err(error) => {
                return Err(error).with_context(|| format!("remove {}", current.display()))
            }
        }
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent.to_path_buf();
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/application_exports.rs"]
mod tests;
