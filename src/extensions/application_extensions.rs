use super::extension_points::ExtensionPoint;
use super::runtime_extensions::{checkout_if_missing, extension_checkout_dir, split_runtime_ref};
use super::AppExtension;
use crate::flatpak_metadata::sections_with_prefix;
use crate::installation::installation_paths::Installation;
use crate::installation::FlatpakApp;
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

pub fn ensure_app_extensions(
    paths: &Installation,
    app: &FlatpakApp,
    app_branch: &str,
) -> Result<Vec<AppExtension>> {
    let metadata_path = app.app_dir.join("metadata");
    let metadata = fs::read_to_string(&metadata_path)
        .with_context(|| format!("read app metadata {}", metadata_path.display()))?;
    let runtime_parts = split_runtime_ref(&app.runtime_ref)?;
    let installed = crate::installation::list_extensions(paths)?;
    let mut extensions = Vec::new();

    for section in sections_with_prefix(&metadata, "Extension ") {
        let point = ExtensionPoint::from_metadata(&metadata, &section, &runtime_parts, app_branch);
        let Some(directory) = point.directory.as_deref() else {
            continue;
        };
        let mut matching = installed
            .iter()
            .filter(|extension| point.keeps_installed_ref(&extension.ref_name))
            .map(|extension| (extension.ref_name.clone(), extension.checkout_dir.clone()))
            .collect::<Vec<_>>();

        // Flatpak may acquire an exact extension point automatically, but a
        // no-autodownload point is only considered after an explicit install.
        if matching.is_empty() && !point.no_autodownload && !point.subdirectories {
            let ref_name = format!(
                "runtime/{}/{}/{}",
                point.name, point.arch, point.preferred_version
            );
            let checkout_dir = extension_checkout_dir(paths, &ref_name)?;
            checkout_if_missing(paths, "extension", &ref_name, None, &checkout_dir, false)?;
            matching.push((ref_name, checkout_dir));
        }

        for (ref_name, checkout_dir) in matching {
            let mut app_mount_relative = PathBuf::from(directory);
            if let Some(subdirectory) = point.mount_subdirectory(&ref_name) {
                app_mount_relative.push(subdirectory);
            }
            let app_mountpoint = app.app_dir.join("files").join(&app_mount_relative);
            fs::create_dir_all(&app_mountpoint).with_context(|| {
                format!(
                    "create app extension mountpoint {}",
                    app_mountpoint.display()
                )
            })?;
            let name = super::runtime_extensions::parse_runtime_ref(&ref_name)
                .map(|parts| parts.name)
                .unwrap_or_else(|| point.name.clone());
            extensions.push(AppExtension {
                name,
                ref_name,
                checkout_dir,
                app_mount_relative,
                ld_library_relative: point.add_ld_path.as_deref().map(PathBuf::from),
            });
        }
    }

    extensions.sort_by(|left, right| {
        left.app_mount_relative
            .components()
            .count()
            .cmp(&right.app_mount_relative.components().count())
            .then_with(|| left.app_mount_relative.cmp(&right.app_mount_relative))
    });
    prepare_nested_mountpoints(&extensions)?;

    Ok(extensions)
}

fn prepare_nested_mountpoints(extensions: &[AppExtension]) -> Result<()> {
    for (parent_index, parent) in extensions.iter().enumerate() {
        for child in extensions.iter().skip(parent_index + 1) {
            let Ok(relative) = child
                .app_mount_relative
                .strip_prefix(&parent.app_mount_relative)
            else {
                continue;
            };
            if relative.as_os_str().is_empty() {
                continue;
            }
            let mountpoint = parent.checkout_dir.join("files").join(relative);
            fs::create_dir_all(&mountpoint).with_context(|| {
                format!(
                    "create nested extension mountpoint {}",
                    mountpoint.display()
                )
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/application_extensions.rs"]
mod tests;
