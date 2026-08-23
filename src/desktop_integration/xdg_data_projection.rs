use super::application_exports::remove_empty_parents;
use crate::installation::installation_paths::Installation;
use anyhow::{bail, Context, Result};
use std::fs;
use std::os::unix::fs as unix_fs;
use std::path::{Component, Path};

pub(super) fn publish_projection(paths: &Installation, rel: &Path) -> Result<()> {
    if !is_launcher_projection(rel) {
        return Ok(());
    }
    let source = paths.export_share().join(rel);
    let target = paths.data_home().join(rel);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create XDG export directory {}", parent.display()))?;
    }

    if let Ok(metadata) = fs::symlink_metadata(&target) {
        if !metadata.file_type().is_symlink() || !is_managed_projection(paths, rel, &target) {
            bail!(
                "refusing to replace existing XDG export {} (source would be {})",
                target.display(),
                source.display()
            );
        }
        fs::remove_file(&target).with_context(|| format!("replace {}", target.display()))?;
    }
    unix_fs::symlink(&source, &target)
        .with_context(|| format!("publish XDG export {}", target.display()))
}

pub(super) fn preflight_projection(paths: &Installation, rel: &Path) -> Result<()> {
    if !is_launcher_projection(rel) {
        return Ok(());
    }
    let target = paths.data_home().join(rel);
    let Ok(metadata) = fs::symlink_metadata(&target) else {
        return Ok(());
    };
    if metadata.file_type().is_symlink() && is_managed_projection(paths, rel, &target) {
        return Ok(());
    }
    bail!(
        "refusing to replace existing XDG export {}",
        target.display()
    )
}

fn is_managed_projection(paths: &Installation, rel: &Path, target: &Path) -> bool {
    let Ok(link) = fs::read_link(target) else {
        return false;
    };
    if link == paths.export_share().join(rel) {
        return true;
    }

    // Older benchmark roots used an overridden data directory and could be
    // deleted while their projections survived. Recognize only the exact
    // project export suffix, never an arbitrary symlink containing the app id.
    let components = link.components().collect::<Vec<_>>();
    components.windows(2).any(|pair| {
        matches!(pair, [Component::Normal(first), Component::Normal(second)]
            if *first == "exports" && *second == "share")
            && link.ends_with(rel)
    })
}

pub(super) fn cleanup_managed_projections_for_app(
    paths: &Installation,
    app_id: &str,
) -> Result<()> {
    for top in ["applications", "icons", "metainfo", "appdata"] {
        let root = paths.data_home().join(top);
        cleanup_managed_projection_tree(paths, app_id, &root)?;
    }
    Ok(())
}

fn cleanup_managed_projection_tree(
    paths: &Installation,
    app_id: &str,
    directory: &Path,
) -> Result<()> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Ok(());
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_dir() {
            cleanup_managed_projection_tree(paths, app_id, &path)?;
            continue;
        }
        if !metadata.file_type().is_symlink()
            || !entry.file_name().to_string_lossy().contains(app_id)
        {
            continue;
        }
        let Ok(rel) = path.strip_prefix(paths.data_home()) else {
            continue;
        };
        if is_managed_projection(paths, rel, &path) {
            fs::remove_file(&path)
                .with_context(|| format!("remove stale XDG export {}", path.display()))?;
        }
    }
    Ok(())
}

pub(super) fn remove_projection(paths: &Installation, rel: &Path, source: &Path) -> Result<()> {
    if !is_launcher_projection(rel) {
        return Ok(());
    }
    let target = paths.data_home().join(rel);
    let Ok(metadata) = fs::symlink_metadata(&target) else {
        return Ok(());
    };
    if metadata.file_type().is_symlink() && fs::read_link(&target).ok().as_deref() == Some(source) {
        fs::remove_file(&target).with_context(|| format!("remove {}", target.display()))?;
        if let Some(parent) = target.parent() {
            remove_empty_parents(paths.data_home(), parent)?;
        }
    }
    Ok(())
}

fn is_launcher_projection(rel: &Path) -> bool {
    matches!(
        rel.components().next(),
        Some(Component::Normal(name))
            if name == "applications" || name == "icons" || name == "metainfo" || name == "appdata"
    )
}
