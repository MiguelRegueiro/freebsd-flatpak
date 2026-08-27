use super::runtime_extensions::{
    checkout_if_missing, first_extension_version, safe_dir_fragment, split_runtime_ref,
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

pub fn ensure_app_codec_extensions(
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
        fs::create_dir_all(&app_mountpoint).with_context(|| {
            format!(
                "create app extension mountpoint {}",
                app_mountpoint.display()
            )
        })?;

        let ref_name = format!(
            "runtime/{}/{}/{}",
            name, runtime_parts.arch, extension_branch
        );
        let checkout_dir = paths.extensions().join(format!(
            "{}-{}",
            safe_dir_fragment(name),
            safe_dir_fragment(&extension_branch)
        ));
        checkout_if_missing(paths, "extension", &ref_name, None, &checkout_dir, false)?;
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
