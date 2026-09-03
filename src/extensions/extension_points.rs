use super::activation::ExtensionFacts;
use crate::flatpak_metadata::{sections_with_prefix, value};
use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExtensionPoint {
    pub(crate) name: String,
    pub(crate) tag: Option<String>,
    pub(crate) directory: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) versions: Vec<String>,
    pub(crate) subdirectories: bool,
    pub(crate) subdirectory_suffix: Option<String>,
    pub(crate) add_ld_path: Option<String>,
    pub(crate) merge_dirs: Vec<String>,
    pub(crate) no_autodownload: bool,
    pub(crate) download_if: Vec<String>,
    pub(crate) enable_if: Vec<String>,
    pub(crate) autodelete: bool,
    pub(crate) autoprune_unless: Vec<String>,
    pub(crate) locale_subset: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExtensionParent {
    pub(crate) arch: String,
    pub(crate) branch: String,
}

impl ExtensionParent {
    pub(crate) fn from_ref(parent_ref: &str) -> Result<Self> {
        let partial = parent_ref
            .strip_prefix("app/")
            .or_else(|| parent_ref.strip_prefix("runtime/"))
            .unwrap_or(parent_ref);
        let mut parts = partial.splitn(3, '/');
        let _name = parts
            .next()
            .context("extension parent ref is missing an ID")?;
        let arch = parts
            .next()
            .filter(|part| !part.is_empty())
            .context("extension parent ref is missing an architecture")?;
        let branch = parts
            .next()
            .filter(|part| !part.is_empty())
            .context("extension parent ref is missing a branch")?;
        Ok(Self {
            arch: arch.to_string(),
            branch: branch.to_string(),
        })
    }
}

impl ExtensionPoint {
    fn parse(metadata: &str, section: &str) -> Self {
        let qualified_name = section.trim_start_matches("Extension ");
        let (name, tag) = qualified_name
            .split_once('@')
            .map_or((qualified_name, None), |(name, tag)| {
                (name, (!tag.is_empty()).then(|| tag.to_string()))
            });
        let related_debug = name.ends_with(".Debug");
        let related_locale = name.ends_with(".Locale");
        Self {
            name: name.to_string(),
            tag,
            directory: value(metadata, section, "directory"),
            version: value(metadata, section, "version").filter(|item| !item.is_empty()),
            versions: list_value(metadata, section, "versions"),
            subdirectories: bool_value(metadata, section, "subdirectories"),
            subdirectory_suffix: value(metadata, section, "subdirectory-suffix")
                .filter(|item| !item.is_empty()),
            add_ld_path: value(metadata, section, "add-ld-path").filter(|item| !item.is_empty()),
            merge_dirs: list_value(metadata, section, "merge-dirs"),
            no_autodownload: related_debug || bool_value(metadata, section, "no-autodownload"),
            download_if: list_value(metadata, section, "download-if"),
            enable_if: list_value(metadata, section, "enable-if"),
            autodelete: related_debug
                || related_locale
                || bool_value(metadata, section, "autodelete"),
            autoprune_unless: list_value(metadata, section, "autoprune-unless"),
            locale_subset: bool_value(metadata, section, "locale-subset"),
        }
    }

    pub(crate) fn branches(&self, parent: &ExtensionParent) -> Vec<String> {
        if !self.versions.is_empty() {
            return deduplicate_preserving_order(self.versions.clone());
        }
        if let Some(version) = &self.version {
            return vec![version.clone()];
        }
        vec![parent.branch.clone()]
    }
}

pub(crate) fn parse_extension_points(metadata: &str) -> Vec<ExtensionPoint> {
    sections_with_prefix(metadata, "Extension ")
        .into_iter()
        .map(|section| ExtensionPoint::parse(metadata, &section))
        .collect()
}

/// Resolve extension points against available runtime refs. `versions` is an
/// ordered compatibility list: for each payload ID the first available branch
/// wins. Tagged points retain separate configuration but resolve the same
/// runtime ID, as in upstream Flatpak.
pub(crate) fn resolve_extension_refs(
    points: &[ExtensionPoint],
    parent: &ExtensionParent,
    available_refs: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut resolved = BTreeSet::new();
    for point in points {
        let mut names = BTreeSet::from([point.name.clone()]);
        if point.subdirectories {
            let prefix = format!("{}.", point.name);
            for candidate in available_refs {
                let Some(parts) = runtime_ref_parts(candidate) else {
                    continue;
                };
                if parts.name.starts_with(&prefix) && parts.arch == parent.arch {
                    names.insert(parts.name.to_string());
                }
            }
        }
        for name in names {
            for branch in point.branches(parent) {
                let candidate = format!("runtime/{name}/{}/{branch}", parent.arch);
                if available_refs.contains(&candidate)
                    && point_for_ref(points, parent, &candidate)
                        .is_some_and(|owner| std::ptr::eq(owner, point))
                {
                    resolved.insert(candidate);
                    break;
                }
            }
        }
    }
    resolved
}

pub(crate) fn point_for_ref<'a>(
    points: &'a [ExtensionPoint],
    parent: &ExtensionParent,
    ref_name: &str,
) -> Option<&'a ExtensionPoint> {
    let candidate = runtime_ref_parts(ref_name)?;
    if candidate.arch != parent.arch {
        return None;
    }
    points
        .iter()
        .filter(|point| {
            (candidate.name == point.name
                || (point.subdirectories
                    && candidate
                        .name
                        .strip_prefix(&point.name)
                        .is_some_and(|suffix| suffix.starts_with('.'))))
                && point
                    .branches(parent)
                    .iter()
                    .any(|branch| branch == candidate.branch)
        })
        // Nested extension points own their payloads. For example, GL.Debug
        // must not also be discovered as a subdirectory of GL.
        .max_by_key(|point| point.name.len())
}

pub(crate) fn required_extension_refs(
    app_dir: &Path,
    app_ref: &str,
    runtime_ref: &str,
    runtime_dir: &Path,
    installed_runtime_refs: &BTreeSet<String>,
    active_gtk_theme: Option<&str>,
) -> Result<BTreeSet<String>> {
    let sources = [
        (app_dir.join("metadata"), app_ref),
        (runtime_dir.join("metadata"), runtime_ref),
    ];
    let mut refs = BTreeSet::new();
    for (metadata_path, parent_ref) in sources {
        refs.extend(applicable_extension_refs(
            &metadata_path,
            parent_ref,
            installed_runtime_refs,
            active_gtk_theme,
        )?);
    }
    Ok(refs)
}

pub(crate) fn applicable_extension_refs(
    metadata_path: &Path,
    parent_ref: &str,
    installed_runtime_refs: &BTreeSet<String>,
    active_gtk_theme: Option<&str>,
) -> Result<BTreeSet<String>> {
    let Ok(metadata) = fs::read_to_string(metadata_path) else {
        return Ok(BTreeSet::new());
    };
    let parent = ExtensionParent::from_ref(parent_ref)?;
    let facts = ExtensionFacts::detect(active_gtk_theme);
    let points = parse_extension_points(&metadata);
    Ok(
        resolve_extension_refs(&points, &parent, installed_runtime_refs)
            .into_iter()
            .filter(|ref_name| {
                point_for_ref(&points, &parent, ref_name)
                    .is_some_and(|point| keeps_installed_ref(point, ref_name, &facts))
            })
            .collect(),
    )
}

pub(crate) fn is_hidden_related_ref(runtime_ref: &str) -> bool {
    runtime_ref.split('/').next().is_some_and(|id| {
        id.split('.')
            .any(|component| matches!(component, "Locale" | "Debug"))
    })
}

pub(crate) fn autodelete_extension_refs(
    app_dir: &Path,
    app_ref: &str,
    runtime_ref: &str,
    runtime_dir: &Path,
    installed_runtime_refs: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    let sources = [
        (
            app_dir.join("metadata"),
            ExtensionParent::from_ref(app_ref)?,
        ),
        (
            runtime_dir.join("metadata"),
            ExtensionParent::from_ref(runtime_ref)?,
        ),
    ];
    let mut refs = BTreeSet::new();
    for (metadata_path, parent) in sources {
        let Ok(metadata) = fs::read_to_string(metadata_path) else {
            continue;
        };
        let points = parse_extension_points(&metadata);
        for ref_name in resolve_extension_refs(&points, &parent, installed_runtime_refs) {
            if point_for_ref(&points, &parent, &ref_name).is_some_and(|point| point.autodelete) {
                refs.insert(ref_name);
            }
        }
    }
    Ok(refs)
}

fn keeps_installed_ref(point: &ExtensionPoint, ref_name: &str, facts: &ExtensionFacts) -> bool {
    let Some(candidate) = runtime_ref_parts(ref_name) else {
        return false;
    };
    if !point.autoprune_unless.is_empty() {
        return facts.matches_any(&point.autoprune_unless, candidate.name);
    }
    let dynamic_conditions = point
        .download_if
        .iter()
        .chain(&point.enable_if)
        .filter(|condition| matches!(condition.as_str(), "active-gl-driver" | "active-gtk-theme"))
        .cloned()
        .collect::<Vec<_>>();
    if !dynamic_conditions.is_empty() {
        return facts.matches_any(&dynamic_conditions, candidate.name);
    }
    true
}

struct RuntimeRefParts<'a> {
    name: &'a str,
    arch: &'a str,
    branch: &'a str,
}

fn runtime_ref_parts(ref_name: &str) -> Option<RuntimeRefParts<'_>> {
    let mut parts = ref_name.strip_prefix("runtime/")?.splitn(3, '/');
    Some(RuntimeRefParts {
        name: parts.next()?,
        arch: parts.next()?,
        branch: parts.next()?,
    })
}

fn list_value(metadata: &str, section: &str, key: &str) -> Vec<String> {
    value(metadata, section, key)
        .into_iter()
        .flat_map(|items| {
            items
                .split(';')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn bool_value(metadata: &str, section: &str, key: &str) -> bool {
    value(metadata, section, key).is_some_and(|item| item.eq_ignore_ascii_case("true"))
}

fn deduplicate_preserving_order(items: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.clone()))
        .collect()
}

#[cfg(test)]
#[path = "tests/extension_points.rs"]
mod tests;
