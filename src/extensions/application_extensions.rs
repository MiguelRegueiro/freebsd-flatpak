use super::runtime_extensions::{
    first_extension_version, split_runtime_ref, validate_extension_checkout,
};
use super::AppExtension;
use crate::flatpak_metadata::{sections_with_prefix, value};
use crate::installation::installation_paths::Installation;
use crate::installation::FlatpakApp;
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

const SUPPORTED_APP_EXTENSIONS: &[&str] = &["org.freedesktop.Platform.ffmpeg-full"];

pub(super) fn is_supported_app_extension(name: &str) -> bool {
    SUPPORTED_APP_EXTENSIONS.contains(&name)
}

pub fn activate_app_codec_extensions(
    paths: &Installation,
    app: &FlatpakApp,
) -> Result<Vec<AppExtension>> {
    let metadata_path = app.app_dir.join("metadata");
    let metadata = fs::read_to_string(&metadata_path)
        .with_context(|| format!("read app metadata {}", metadata_path.display()))?;
    let runtime_parts = split_runtime_ref(&app.runtime_ref)?;
    let mut extensions = Vec::new();

    for section in sections_with_prefix(&metadata, "Extension ") {
        let name = section.trim_start_matches("Extension ");
        if !is_supported_app_extension(name) {
            continue;
        }

        let Some(directory) = value(&metadata, &section, "directory") else {
            continue;
        };
        let extension_branch = value(&metadata, &section, "version")
            .or_else(|| {
                value(&metadata, &section, "versions")
                    .and_then(|versions| first_extension_version(&versions))
            })
            .unwrap_or_else(|| runtime_parts.branch.clone());
        let app_mount_relative = PathBuf::from(directory);
        let app_mountpoint = app.app_dir.join("files").join(&app_mount_relative);
        if !app_mountpoint.is_dir() {
            anyhow::bail!(
                "required app extension mountpoint is missing at {}; run `flatpak update` or `flatpak repair`",
                app_mountpoint.display()
            );
        }

        let ref_name = format!(
            "runtime/{}/{}/{}",
            name, runtime_parts.arch, extension_branch
        );
        let partial_ref = ref_name
            .strip_prefix("runtime/")
            .expect("runtime extension ref");
        let checkout_dir = crate::installation::get_runtime(paths, partial_ref)?
            .map(|record| crate::installation::absolute(paths, &record.runtime_dir))
            .unwrap_or_else(|| {
                paths
                    .runtimes()
                    .join(super::runtime_extensions::runtime_checkout_dir(partial_ref))
            });
        validate_extension_checkout(&ref_name, &checkout_dir)?;
        let ld_library_relative = value(&metadata, &section, "add-ld-path")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from);

        extensions.push(AppExtension {
            name: name.to_string(),
            ref_name,
            checkout_dir,
            app_mount_relative,
            ld_library_relative,
        });
    }

    Ok(extensions)
}

#[cfg(test)]
#[path = "tests/application_extensions.rs"]
mod tests;
