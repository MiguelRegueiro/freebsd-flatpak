use super::extension_points::{
    parse_extension_points, resolve_extension_refs, ExtensionParent, ExtensionPoint,
};
use super::runtime_extensions::{valid_relative_extension_path, validate_extension_checkout};
use crate::flatpak_metadata::value;
use crate::installation::installation_paths::Installation;
use crate::installation::{self as state, FlatpakApp};
use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ExtensionFacts {
    pub(crate) active_gl_drivers: BTreeSet<String>,
    pub(crate) active_gtk_theme: Option<String>,
    pub(crate) intel_gpu: bool,
    pub(crate) kernel_modules: BTreeSet<String>,
    pub(crate) xdg_desktops: BTreeSet<String>,
}

impl ExtensionFacts {
    pub(crate) fn detect(active_gtk_theme: Option<&str>) -> Self {
        let active_gl_drivers = std::env::var("FLATPAK_GL_DRIVERS")
            .ok()
            .into_iter()
            .flat_map(|drivers| split_fact_values(&drivers))
            .chain(std::iter::once("default".to_string()))
            .collect();
        let xdg_desktops = std::env::var("XDG_CURRENT_DESKTOP")
            .ok()
            .into_iter()
            .flat_map(|desktops| split_fact_values(&desktops))
            .map(|desktop| desktop.to_ascii_lowercase())
            .collect();
        let kernel_modules = Command::new("kldstat")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|output| {
                output
                    .lines()
                    .filter_map(|line| line.split_whitespace().last())
                    .map(normalize_kernel_module)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            active_gl_drivers,
            active_gtk_theme: active_gtk_theme.map(ToOwned::to_owned),
            intel_gpu: crate::host_resources::video_acceleration::host_has_intel_drm_device(),
            kernel_modules,
            xdg_desktops,
        }
    }

    pub(crate) fn matches_any(&self, conditions: &[String], extension_name: &str) -> bool {
        conditions
            .iter()
            .any(|condition| self.matches(condition, extension_name))
    }

    fn matches(&self, condition: &str, extension_name: &str) -> bool {
        match condition {
            "active-gl-driver" => extension_suffix_matches(extension_name, &self.active_gl_drivers),
            "active-gtk-theme" => self.active_gtk_theme.as_deref().is_some_and(|theme| {
                extension_name
                    .rsplit_once('.')
                    .is_some_and(|(_, suffix)| suffix == theme)
            }),
            "have-intel-gpu" => self.intel_gpu,
            _ => {
                condition
                    .strip_prefix("have-kernel-module-")
                    .is_some_and(|module| {
                        self.kernel_modules
                            .contains(&normalize_kernel_module(module))
                    })
                    || condition
                        .strip_prefix("on-xdg-desktop-")
                        .is_some_and(|desktop| {
                            self.xdg_desktops.contains(&desktop.to_ascii_lowercase())
                        })
            }
        }
    }
}

fn split_fact_values(value: &str) -> Vec<String> {
    value
        .split([':', ';', ','])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn normalize_kernel_module(module: &str) -> String {
    module
        .trim_end_matches(".ko")
        .replace('-', "_")
        .to_ascii_lowercase()
}

fn extension_suffix_matches(name: &str, active: &BTreeSet<String>) -> bool {
    active.iter().any(|driver| {
        name == driver
            || name
                .strip_suffix(driver)
                .is_some_and(|prefix| prefix.ends_with('.'))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExtensionScope {
    App,
    Runtime,
}

impl ExtensionScope {
    fn root(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Runtime => "usr",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExtensionMount {
    pub(crate) name: String,
    pub(crate) ref_name: String,
    pub(crate) commit: String,
    pub(crate) checkout_dir: PathBuf,
    pub(crate) target: PathBuf,
    pub(crate) add_ld_paths: Vec<String>,
    pub(crate) merge_dirs: Vec<ExtensionMergeMount>,
    pub(crate) priority: i32,
    pub(crate) scope: ExtensionScope,
    pub(crate) conditions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExtensionMergeMount {
    pub(crate) target: PathBuf,
    pub(crate) source_relative: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ExtensionMountPlan {
    pub(crate) mounts: Vec<ExtensionMount>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExtensionMergeDirectory {
    pub(crate) target: PathBuf,
    pub(crate) base_source: PathBuf,
    pub(crate) entries: Vec<ExtensionMergeEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExtensionMergeEntry {
    pub(crate) name: PathBuf,
    pub(crate) source: PathBuf,
}

impl ExtensionMountPlan {
    pub(crate) fn refs(&self) -> Vec<String> {
        self.mounts
            .iter()
            .map(|mount| mount.ref_name.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub(crate) fn checkout_dirs(&self) -> Vec<PathBuf> {
        self.mounts
            .iter()
            .map(|mount| mount.checkout_dir.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub(crate) fn app_ld_library_paths(&self) -> Vec<String> {
        self.mounts
            .iter()
            .filter(|mount| mount.scope == ExtensionScope::App)
            .flat_map(|mount| mount.add_ld_paths.iter().cloned())
            .collect()
    }

    pub(crate) fn runtime_ld_library_paths(&self) -> Vec<String> {
        self.mounts
            .iter()
            .filter(|mount| mount.scope == ExtensionScope::Runtime)
            .flat_map(|mount| mount.add_ld_paths.iter().cloned())
            .collect()
    }

    pub(crate) fn conditioned_mount(&self, condition: &str) -> Option<&ExtensionMount> {
        self.mounts
            .iter()
            .find(|mount| mount.conditions.iter().any(|item| item == condition))
    }

    pub(crate) fn app_info(&self) -> Vec<String> {
        extension_info(
            self.mounts
                .iter()
                .filter(|mount| mount.scope == ExtensionScope::App),
        )
    }

    pub(crate) fn runtime_info(&self) -> Vec<String> {
        extension_info(
            self.mounts
                .iter()
                .filter(|mount| mount.scope == ExtensionScope::Runtime),
        )
    }

    pub(crate) fn merge_directories(
        &self,
        app_dir: &Path,
        runtime_dir: &Path,
    ) -> Result<Vec<ExtensionMergeDirectory>> {
        #[derive(Clone)]
        struct Candidate {
            priority: i32,
            ref_name: String,
            source: PathBuf,
        }

        let mut groups = BTreeMap::<PathBuf, (PathBuf, BTreeMap<PathBuf, Candidate>)>::new();
        for mount in &self.mounts {
            for merge in &mount.merge_dirs {
                let relative = merge
                    .target
                    .strip_prefix(mount.scope.root())
                    .expect("merge target starts with scope root");
                let base_source = match mount.scope {
                    ExtensionScope::App => app_dir,
                    ExtensionScope::Runtime => runtime_dir,
                }
                .join("files")
                .join(relative);
                let extension_source = mount
                    .checkout_dir
                    .join("files")
                    .join(&merge.source_relative);
                let group = groups
                    .entry(merge.target.clone())
                    .or_insert_with(|| (base_source, BTreeMap::new()));
                let Ok(entries) = fs::read_dir(&extension_source) else {
                    continue;
                };
                for entry in entries {
                    let entry = entry.with_context(|| {
                        format!("read merge directory {}", extension_source.display())
                    })?;
                    let name = PathBuf::from(entry.file_name());
                    let candidate = Candidate {
                        priority: mount.priority,
                        ref_name: mount.ref_name.clone(),
                        source: entry.path(),
                    };
                    let replace = group.1.get(&name).is_none_or(|current| {
                        candidate.priority > current.priority
                            || (candidate.priority == current.priority
                                && candidate.ref_name < current.ref_name)
                    });
                    if replace {
                        group.1.insert(name, candidate);
                    }
                }
            }
        }
        let mut directories = groups
            .into_iter()
            .map(|(target, (base_source, entries))| ExtensionMergeDirectory {
                target,
                base_source,
                entries: entries
                    .into_iter()
                    .map(|(name, candidate)| ExtensionMergeEntry {
                        name,
                        source: candidate.source,
                    })
                    .collect(),
            })
            .collect::<Vec<_>>();
        directories.sort_by(|left, right| {
            path_depth(&left.target)
                .cmp(&path_depth(&right.target))
                .then_with(|| left.target.cmp(&right.target))
        });
        Ok(directories)
    }
}

fn extension_info<'a>(mounts: impl Iterator<Item = &'a ExtensionMount>) -> Vec<String> {
    mounts
        .map(|mount| format!("{}={}", mount.name, mount.commit))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(crate) fn resolve_extension_mount_plan(
    paths: &Installation,
    app: &FlatpakApp,
    facts: &ExtensionFacts,
) -> Result<ExtensionMountPlan> {
    let app_ref = state::deployment_marker(&app.app_dir)?
        .map(|(ref_name, _)| ref_name)
        .unwrap_or_else(|| {
            let parent = ExtensionParent::from_ref(&app.runtime_ref)
                .expect("validated runtime ref has architecture and branch");
            format!("app/{}/{}/{}", app.app_id, parent.arch, parent.branch)
        });
    let installed = state::list_runtimes(paths)?;
    let available = installed
        .iter()
        .map(|runtime| format!("runtime/{}", runtime.runtime_ref))
        .collect::<BTreeSet<_>>();
    let installed = installed
        .into_iter()
        .map(|runtime| (format!("runtime/{}", runtime.runtime_ref), runtime))
        .collect::<BTreeMap<_, _>>();
    let mut mounts = Vec::new();
    resolve_metadata_mounts(
        &app.runtime_dir,
        &app.runtime_ref,
        ExtensionScope::Runtime,
        facts,
        &available,
        &installed,
        paths,
        &mut mounts,
    )?;
    resolve_metadata_mounts(
        &app.app_dir,
        &app_ref,
        ExtensionScope::App,
        facts,
        &available,
        &installed,
        paths,
        &mut mounts,
    )?;
    mounts.sort_by(|left, right| {
        path_depth(&left.target)
            .cmp(&path_depth(&right.target))
            .then_with(|| left.target.cmp(&right.target))
            .then_with(|| right.priority.cmp(&left.priority))
            .then_with(|| left.ref_name.cmp(&right.ref_name))
    });
    Ok(ExtensionMountPlan { mounts })
}

#[allow(clippy::too_many_arguments)]
fn resolve_metadata_mounts(
    parent_dir: &Path,
    parent_ref: &str,
    scope: ExtensionScope,
    facts: &ExtensionFacts,
    available: &BTreeSet<String>,
    installed: &BTreeMap<String, state::RuntimeRecord>,
    paths: &Installation,
    mounts: &mut Vec<ExtensionMount>,
) -> Result<()> {
    let metadata_path = parent_dir.join("metadata");
    let metadata = fs::read_to_string(&metadata_path)
        .with_context(|| format!("read extension metadata {}", metadata_path.display()))?;
    let parent = ExtensionParent::from_ref(parent_ref)?;
    let points = parse_extension_points(&metadata);
    for point in &points {
        for ref_name in resolve_extension_refs(std::slice::from_ref(point), &parent, available) {
            if !launch_enabled(point, &ref_name, facts) {
                continue;
            }
            let Some(record) = installed.get(&ref_name) else {
                continue;
            };
            let checkout_dir = state::absolute(paths, &record.runtime_dir);
            validate_extension_checkout(&ref_name, &checkout_dir)?;
            let target = mount_target(point, &ref_name, scope)?;
            let extension_metadata =
                fs::read_to_string(checkout_dir.join("metadata")).unwrap_or_default();
            let priority = value(&extension_metadata, "ExtensionOf", "priority")
                .and_then(|priority| priority.parse().ok())
                .unwrap_or(0);
            let add_ld_paths = point
                .add_ld_path
                .iter()
                .flat_map(|paths| paths.split(':'))
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(|path| {
                    if !valid_relative_extension_path(path) {
                        bail!("invalid add-ld-path for {}: {path:?}", point.name);
                    }
                    Ok(PathBuf::from("/")
                        .join(&target)
                        .join(path)
                        .display()
                        .to_string())
                })
                .collect::<Result<Vec<_>>>()?;
            let merge_dirs = point
                .merge_dirs
                .iter()
                .map(|dir| {
                    if !valid_relative_extension_path(dir) {
                        bail!("invalid merge-dirs path for {}: {dir:?}", point.name);
                    }
                    Ok(ExtensionMergeMount {
                        target: PathBuf::from(scope.root())
                            .join(
                                point
                                    .directory
                                    .as_deref()
                                    .expect("mount target already required a directory"),
                            )
                            .join(dir),
                        source_relative: PathBuf::from(dir),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let mut conditions = point.enable_if.clone();
            conditions.extend(point.download_if.clone());
            conditions.retain(|condition| {
                facts.matches_any(
                    std::slice::from_ref(condition),
                    runtime_name(&ref_name).unwrap_or_default(),
                )
            });
            conditions.sort();
            conditions.dedup();
            mounts.push(ExtensionMount {
                name: runtime_name(&ref_name).unwrap_or(&point.name).to_string(),
                ref_name,
                commit: record.runtime_commit.clone(),
                checkout_dir,
                target,
                add_ld_paths,
                merge_dirs,
                priority,
                scope,
                conditions,
            });
        }
    }
    Ok(())
}

fn launch_enabled(point: &ExtensionPoint, ref_name: &str, facts: &ExtensionFacts) -> bool {
    point.enable_if.is_empty()
        || facts.matches_any(&point.enable_if, runtime_name(ref_name).unwrap_or_default())
}

fn mount_target(point: &ExtensionPoint, ref_name: &str, scope: ExtensionScope) -> Result<PathBuf> {
    let directory = point
        .directory
        .as_deref()
        .context("extension point is missing directory")?;
    if !valid_relative_extension_path(directory) {
        bail!(
            "invalid extension directory for {}: {directory:?}",
            point.name
        );
    }
    let mut target = PathBuf::from(scope.root()).join(directory);
    if point.subdirectories {
        let name = runtime_name(ref_name).unwrap_or_default();
        let suffix = name
            .strip_prefix(&format!("{}.", point.name))
            .context("subdirectory extension ref does not extend its point name")?;
        target.push(suffix);
    }
    if let Some(suffix) = &point.subdirectory_suffix {
        if !valid_relative_extension_path(suffix) {
            bail!("invalid extension subdirectory suffix: {suffix:?}");
        }
        target.push(suffix);
    }
    Ok(target)
}

fn runtime_name(ref_name: &str) -> Option<&str> {
    ref_name.strip_prefix("runtime/")?.split('/').next()
}

fn path_depth(path: &Path) -> usize {
    path.components().count()
}

#[cfg(test)]
#[path = "tests/activation.rs"]
mod tests;
