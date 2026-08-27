use super::runtime_extensions::{parse_runtime_ref, split_runtime_ref, RuntimeRefParts};
use crate::flatpak_metadata::{sections_with_prefix, value};
use anyhow::Result;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

pub fn required_extension_refs(
    app_dir: &Path,
    runtime_ref: &str,
    runtime_dir: &Path,
    installed_extension_refs: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    let parts = split_runtime_ref(runtime_ref)?;
    let mut refs = BTreeSet::new();
    for metadata_path in [app_dir.join("metadata"), runtime_dir.join("metadata")] {
        let Ok(metadata) = fs::read_to_string(&metadata_path) else {
            continue;
        };
        for section in sections_with_prefix(&metadata, "Extension ") {
            let point = ExtensionPoint::from_metadata(&metadata, &section, &parts);
            refs.extend(
                installed_extension_refs
                    .iter()
                    .filter(|ref_name| point.keeps_installed_ref(ref_name))
                    .cloned(),
            );
        }
    }
    Ok(refs)
}

struct ExtensionPoint {
    name: String,
    arch: String,
    versions: BTreeSet<String>,
    subdirectories: bool,
    active_gl_driver_condition: bool,
    autoprune_unless_active_gl_driver: bool,
}

impl ExtensionPoint {
    fn from_metadata(metadata: &str, section: &str, runtime: &RuntimeRefParts) -> Self {
        let versions = value(metadata, section, "version")
            .into_iter()
            .chain(
                value(metadata, section, "versions")
                    .into_iter()
                    .flat_map(|versions| {
                        versions
                            .split(';')
                            .map(str::trim)
                            .filter(|version| !version.is_empty())
                            .map(ToOwned::to_owned)
                            .collect::<Vec<_>>()
                    }),
            )
            .collect::<BTreeSet<_>>();
        let versions = if versions.is_empty() {
            BTreeSet::from([runtime.branch.clone()])
        } else {
            versions
        };
        let condition_is_active_gl_driver = |key| {
            value(metadata, section, key).is_some_and(|value| {
                value
                    .split(';')
                    .map(str::trim)
                    .any(|v| v == "active-gl-driver")
            })
        };
        Self {
            name: section.trim_start_matches("Extension ").to_string(),
            arch: runtime.arch.clone(),
            versions,
            subdirectories: value(metadata, section, "subdirectories")
                .is_some_and(|value| value == "true"),
            active_gl_driver_condition: condition_is_active_gl_driver("download-if")
                || condition_is_active_gl_driver("enable-if"),
            autoprune_unless_active_gl_driver: value(metadata, section, "autoprune-unless")
                .is_some_and(|value| {
                    value
                        .split(';')
                        .map(str::trim)
                        .any(|item| item == "active-gl-driver")
                }),
        }
    }

    fn keeps_installed_ref(&self, ref_name: &str) -> bool {
        let Some(candidate) = parse_runtime_ref(ref_name) else {
            return false;
        };
        let name_matches = candidate.name == self.name
            || ((self.subdirectories || self.active_gl_driver_condition)
                && candidate
                    .name
                    .strip_prefix(&self.name)
                    .is_some_and(|suffix| suffix.starts_with('.') && suffix.len() > 1));
        if !name_matches
            || candidate.arch != self.arch
            || !self.versions.contains(&candidate.branch)
        {
            return false;
        }

        if self.active_gl_driver_condition || self.autoprune_unless_active_gl_driver {
            return candidate.name == format!("{}.default", self.name);
        }
        true
    }
}

#[cfg(test)]
#[path = "tests/extension_points.rs"]
mod tests;
