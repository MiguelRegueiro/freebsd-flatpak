use super::application_extensions::is_supported_app_extension;
use super::runtime_extensions::{first_extension_version, safe_dir_fragment, split_runtime_ref};
use crate::flatpak_metadata::{has_section, sections_with_prefix, value};
use crate::installation::installation_paths::Installation;
use crate::installation::AppRecord;
use crate::ostree::{Deployment, RemoteSource, Storage, StorageTimings};
use crate::remotes::{self, RemoteMetadata};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const GL_EXTENSION: &str = "org.freedesktop.Platform.GL";
const INTEL_VAAPI_EXTENSION: &str = "org.freedesktop.Platform.VAAPI.Intel";

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequiredExtension {
    ref_name: String,
    checkout_dir: PathBuf,
    preferred_origin: String,
}

struct CachedRemote {
    metadata: RemoteMetadata,
    summary: Vec<u8>,
}

struct ResolvedExtension {
    requirement: RequiredExtension,
    origin: String,
    checksum: String,
}

/// Reconcile every extension needed by the selected installed applications in
/// one deployment transaction. Remote summaries are loaded, parsed, and read
/// at most once for the lifetime of this reconciliation.
pub(crate) fn reconcile_extensions(
    paths: &Installation,
    apps: &[AppRecord],
    force: bool,
) -> Result<StorageTimings> {
    reconcile_extensions_with_metadata(paths, apps, force, Vec::new(), true)
}

pub(crate) fn reconcile_extensions_with_metadata(
    paths: &Installation,
    apps: &[AppRecord],
    force: bool,
    preloaded: Vec<RemoteMetadata>,
    show_reused: bool,
) -> Result<StorageTimings> {
    if apps.is_empty() {
        return Ok(StorageTimings::default());
    }

    let intel_vaapi = crate::host_resources::video_acceleration::host_has_intel_drm_device();
    let mut requirements = BTreeMap::new();
    for app in apps {
        for requirement in required_for_app(paths, app, intel_vaapi)? {
            requirements
                .entry(requirement.ref_name.clone())
                .or_insert(requirement);
        }
    }
    if requirements.is_empty() {
        return Ok(StorageTimings::default());
    }

    let mut enabled = remotes::enabled_remotes(paths)?;
    enabled.sort_by_key(|remote| (remote.name != remotes::DEFAULT_REMOTE, remote.name.clone()));
    let enabled_names = enabled
        .into_iter()
        .map(|remote| remote.name)
        .collect::<Vec<_>>();
    let mut cache = BTreeMap::<String, CachedRemote>::new();
    for metadata in preloaded {
        let summary = metadata.summary_bytes()?;
        cache.insert(
            metadata.remote_name().to_string(),
            CachedRemote { metadata, summary },
        );
    }
    let mut unavailable = BTreeMap::<String, String>::new();
    let mut resolved = Vec::new();

    for requirement in requirements.into_values() {
        let mut origins = Vec::new();
        if let Some(origin) = deployment_origin(&requirement.checkout_dir, &requirement.ref_name) {
            origins.push(origin);
        }
        origins.push(requirement.preferred_origin.clone());
        origins.extend(enabled_names.iter().cloned());
        let mut seen = BTreeSet::new();
        origins.retain(|origin| seen.insert(origin.clone()));

        let mut selected = None;
        for origin in origins {
            if !enabled_names.contains(&origin) {
                continue;
            }
            if unavailable.contains_key(&origin) {
                continue;
            }
            if !cache.contains_key(&origin) {
                let loaded = remotes::load_remote_metadata(paths, &origin).and_then(|metadata| {
                    let summary = metadata.summary_bytes()?;
                    Ok(CachedRemote { metadata, summary })
                });
                match loaded {
                    Ok(remote) => {
                        cache.insert(origin.clone(), remote);
                    }
                    Err(error) => {
                        unavailable.insert(origin.clone(), format!("{error:#}"));
                        continue;
                    }
                }
            }
            let remote = cache.get(&origin).expect("inserted remote metadata");
            if let Some(checksum) = remote.metadata.ref_checksum(&requirement.ref_name) {
                selected = Some((
                    remote.metadata.remote_name().to_string(),
                    checksum.to_string(),
                ));
                break;
            }
        }
        let (origin, checksum) = selected.with_context(|| {
            let failures = unavailable
                .iter()
                .map(|(origin, error)| format!("{origin}: {error}"))
                .collect::<Vec<_>>()
                .join("; ");
            if failures.is_empty() {
                format!(
                    "ref is not present in an enabled remote: {}",
                    requirement.ref_name
                )
            } else {
                format!(
                    "ref is not present in available remote metadata: {} ({failures})",
                    requirement.ref_name
                )
            }
        })?;
        resolved.push(ResolvedExtension {
            requirement,
            origin,
            checksum,
        });
    }

    let used_origins = resolved
        .iter()
        .map(|extension| extension.origin.as_str())
        .collect::<BTreeSet<_>>();
    let sources = used_origins
        .iter()
        .map(|origin| {
            let cached = cache.get(*origin).expect("resolved remote is cached");
            RemoteSource {
                name: origin,
                summary: &cached.summary,
            }
        })
        .collect::<Vec<_>>();
    let deployments = resolved
        .iter()
        .map(|extension| Deployment {
            remote: &extension.origin,
            kind: "extension",
            ref_name: &extension.requirement.ref_name,
            checksum: &extension.checksum,
            destination: &extension.requirement.checkout_dir,
            force,
        })
        .collect::<Vec<_>>();
    Storage::open(paths)?.deploy_from_sources_with_reuse_output(&sources, &deployments, show_reused)
}

fn required_for_app(
    paths: &Installation,
    app: &AppRecord,
    intel_vaapi: bool,
) -> Result<Vec<RequiredExtension>> {
    let app_dir = crate::installation::absolute(paths, &app.app_dir);
    let runtime_dir =
        crate::installation::get_runtime_from(paths, &app.runtime_origin, &app.runtime_ref)?
            .map(|runtime| crate::installation::absolute(paths, &runtime.runtime_dir))
            .unwrap_or_else(|| crate::installation::absolute(paths, &app.runtime_dir));
    let runtime_metadata_path = runtime_dir.join("metadata");
    let runtime_metadata = fs::read_to_string(&runtime_metadata_path)
        .with_context(|| format!("read runtime metadata {}", runtime_metadata_path.display()))?;
    let runtime = split_runtime_ref(&app.runtime_ref)?;
    let mut requirements = Vec::new();

    let gl_section = format!("Extension {GL_EXTENSION}");
    if has_section(&runtime_metadata, &gl_section) {
        let branch = value(&runtime_metadata, &gl_section, "versions")
            .and_then(|versions| first_extension_version(&versions))
            .unwrap_or_else(|| runtime.branch.clone());
        ensure_mountpoint(
            &runtime_dir,
            value(&runtime_metadata, &gl_section, "directory")
                .as_deref()
                .unwrap_or("lib/x86_64-linux-gnu/GL"),
            Some("default"),
        )?;
        requirements.push(requirement(
            paths,
            &format!("{GL_EXTENSION}.default"),
            &runtime.arch,
            &branch,
            &app.runtime_origin,
        ));
    }

    if intel_vaapi {
        let section = format!("Extension {INTEL_VAAPI_EXTENSION}");
        if has_section(&runtime_metadata, &section) {
            let branch = extension_branch(&runtime_metadata, &section, &runtime.branch);
            ensure_mountpoint(
                &runtime_dir,
                value(&runtime_metadata, &section, "directory")
                    .as_deref()
                    .unwrap_or("lib/x86_64-linux-gnu/dri/intel-vaapi-driver"),
                None,
            )?;
            requirements.push(requirement(
                paths,
                INTEL_VAAPI_EXTENSION,
                &runtime.arch,
                &branch,
                &app.runtime_origin,
            ));
        }
    }

    let app_metadata_path = app_dir.join("metadata");
    let app_metadata = fs::read_to_string(&app_metadata_path)
        .with_context(|| format!("read app metadata {}", app_metadata_path.display()))?;
    for section in sections_with_prefix(&app_metadata, "Extension ") {
        let name = section.trim_start_matches("Extension ");
        if !is_supported_app_extension(name) {
            continue;
        }
        let Some(directory) = value(&app_metadata, &section, "directory") else {
            continue;
        };
        let branch = extension_branch(&app_metadata, &section, &runtime.branch);
        ensure_mountpoint(&app_dir, &directory, None)?;
        requirements.push(requirement(
            paths,
            name,
            &runtime.arch,
            &branch,
            &app.origin,
        ));
    }

    Ok(requirements)
}

fn extension_branch(metadata: &str, section: &str, fallback: &str) -> String {
    value(metadata, section, "version")
        .or_else(|| {
            value(metadata, section, "versions")
                .and_then(|versions| first_extension_version(&versions))
        })
        .unwrap_or_else(|| fallback.to_string())
}

fn requirement(
    paths: &Installation,
    name: &str,
    arch: &str,
    branch: &str,
    preferred_origin: &str,
) -> RequiredExtension {
    RequiredExtension {
        ref_name: format!("runtime/{name}/{arch}/{branch}"),
        checkout_dir: paths.extensions().join(format!(
            "{}-{}",
            safe_dir_fragment(name),
            safe_dir_fragment(branch)
        )),
        preferred_origin: preferred_origin.to_string(),
    }
}

fn deployment_origin(checkout_dir: &Path, expected_ref: &str) -> Option<String> {
    let marker = fs::read_to_string(checkout_dir.join(".ostree-commit")).ok()?;
    let mut lines = marker.lines();
    if lines.next()? != expected_ref {
        return None;
    }
    let _commit = lines.next()?;
    let _installed_size = lines.next()?;
    lines.next().map(ToOwned::to_owned)
}

fn ensure_mountpoint(root: &Path, directory: &str, suffix: Option<&str>) -> Result<()> {
    let mut mountpoint = root.join("files").join(directory);
    if let Some(suffix) = suffix {
        mountpoint.push(suffix);
    }
    fs::create_dir_all(&mountpoint)
        .with_context(|| format!("create extension mountpoint {}", mountpoint.display()))
}

#[cfg(test)]
#[path = "tests/reconciliation.rs"]
mod tests;
