use super::activation::ExtensionFacts;
use super::extension_points::{
    parse_extension_points, resolve_extension_refs, ExtensionParent, ExtensionPoint,
};
use super::runtime_extensions::valid_relative_extension_path;
use crate::installation::installation_paths::Installation;
use crate::installation::{AppRecord, RuntimeRecord};
use crate::ostree::{Deployment, RemoteSource, Storage, StorageTimings};
use crate::remotes::{self, RemoteMetadata};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequiredExtension {
    ref_name: String,
    preferred_origin: String,
    optional: bool,
}

struct CachedRemote {
    metadata: RemoteMetadata,
    summary: Vec<u8>,
}

struct ResolvedExtension {
    requirement: RequiredExtension,
    origin: String,
    checksum: String,
    destination: PathBuf,
    explicitly_installed: bool,
}

/// Reconcile extension payloads as ordinary runtime refs. Remote summaries are
/// loaded once and shared by generic extension-point discovery and deployment.
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
    for origin in &enabled_names {
        if cache.contains_key(origin) {
            continue;
        }
        match remotes::load_remote_metadata(paths, origin).and_then(|metadata| {
            let summary = metadata.summary_bytes()?;
            Ok(CachedRemote { metadata, summary })
        }) {
            Ok(remote) => {
                cache.insert(origin.clone(), remote);
            }
            Err(error) => {
                unavailable.insert(origin.clone(), format!("{error:#}"));
            }
        }
    }

    let installed = crate::installation::list_runtimes(paths)?;
    let mut available_refs = cache
        .values()
        .flat_map(|remote| remote.metadata.list_refs())
        .map(|item| item.ref_name)
        .filter(|ref_name| ref_name.starts_with("runtime/"))
        .collect::<BTreeSet<_>>();
    available_refs.extend(
        installed
            .iter()
            .map(|runtime| format!("runtime/{}", runtime.runtime_ref)),
    );

    let gtk_theme = crate::host_resources::cursor_themes::active_gtk_theme();
    let facts = ExtensionFacts::detect(gtk_theme.as_deref());
    let installed_refs = installed
        .iter()
        .map(|runtime| format!("runtime/{}", runtime.runtime_ref))
        .collect::<BTreeSet<_>>();
    let mut requirements = BTreeMap::new();
    for app in apps {
        for requirement in required_for_app(paths, app, &available_refs, &facts, &installed_refs)? {
            requirements
                .entry(requirement.ref_name.clone())
                .or_insert(requirement);
        }
    }
    if requirements.is_empty() {
        return Ok(StorageTimings::default());
    }

    let installed_by_ref = installed
        .into_iter()
        .map(|record| (format!("runtime/{}", record.runtime_ref), record))
        .collect::<BTreeMap<_, _>>();
    let mut resolved = Vec::new();
    for requirement in requirements.into_values() {
        let existing = installed_by_ref.get(&requirement.ref_name);
        let mut origins = Vec::new();
        if let Some(record) = existing {
            origins.push(record.origin.clone());
        }
        origins.push(requirement.preferred_origin.clone());
        origins.extend(enabled_names.iter().cloned());
        let mut seen = BTreeSet::new();
        origins.retain(|origin| seen.insert(origin.clone()));

        let selected = origins.into_iter().find_map(|origin| {
            cache
                .get(&origin)
                .and_then(|remote| remote.metadata.ref_checksum(&requirement.ref_name))
                .map(|checksum| (origin, checksum.to_string()))
        });
        let Some((origin, checksum)) = selected else {
            if existing.is_some_and(|record| runtime_checkout_is_valid(paths, record))
                || requirement.optional
            {
                continue;
            }
            let failures = unavailable
                .iter()
                .map(|(origin, error)| format!("{origin}: {error}"))
                .collect::<Vec<_>>()
                .join("; ");
            anyhow::bail!(if failures.is_empty() {
                format!(
                    "ref is not present in an enabled remote: {}",
                    requirement.ref_name
                )
            } else {
                format!(
                    "ref is not present in available remote metadata: {} ({failures})",
                    requirement.ref_name
                )
            });
        };
        let (destination, explicitly_installed) = runtime_destination(
            paths,
            &requirement.ref_name,
            &origin,
            &checksum,
            existing,
            force,
        );
        resolved.push(ResolvedExtension {
            requirement,
            origin,
            checksum,
            destination,
            explicitly_installed,
        });
    }
    if resolved.is_empty() {
        return Ok(StorageTimings::default());
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
            kind: "runtime",
            ref_name: &extension.requirement.ref_name,
            checksum: &extension.checksum,
            destination: &extension.destination,
            force,
        })
        .collect::<Vec<_>>();
    let storage = Storage::open(paths)?;
    let timings =
        storage.deploy_from_sources_with_reuse_output(&sources, &deployments, show_reused)?;
    for extension in resolved {
        let partial_ref = extension
            .requirement
            .ref_name
            .strip_prefix("runtime/")
            .expect("extension payload is a runtime ref");
        crate::installation::write_runtime(
            paths,
            &RuntimeRecord {
                origin: extension.origin,
                runtime_ref: partial_ref.to_string(),
                runtime_commit: extension.checksum.clone(),
                installed_size: storage.installed_size(&extension.checksum)?,
                explicitly_installed: extension.explicitly_installed,
                runtime_dir: paths.relative_data_path(&extension.destination)?,
            },
        )?;
    }
    drop(storage);
    crate::installation::reconcile_runtime_bindings(paths)?;
    Ok(timings)
}

fn runtime_destination(
    paths: &Installation,
    ref_name: &str,
    origin: &str,
    checksum: &str,
    existing: Option<&RuntimeRecord>,
    force: bool,
) -> (PathBuf, bool) {
    if let Some(record) = existing.filter(|record| {
        record.origin == origin
            && record.runtime_commit == checksum
            && runtime_checkout_is_valid(paths, record)
    }) {
        return (
            crate::installation::absolute(paths, &record.runtime_dir),
            record.explicitly_installed,
        );
    }
    let partial_ref = ref_name
        .strip_prefix("runtime/")
        .expect("extension payload is a runtime ref");
    (
        crate::installation::generation_checkout_dir(
            &paths
                .runtimes()
                .join(super::runtime_checkout_dir(partial_ref)),
            checksum,
            force,
        ),
        existing.is_some_and(|record| record.explicitly_installed),
    )
}

fn runtime_checkout_is_valid(paths: &Installation, record: &RuntimeRecord) -> bool {
    let directory = crate::installation::absolute(paths, &record.runtime_dir);
    directory.is_dir() && directory.join("files").is_dir()
}

fn required_for_app(
    paths: &Installation,
    app: &AppRecord,
    available_refs: &BTreeSet<String>,
    facts: &ExtensionFacts,
    installed_refs: &BTreeSet<String>,
) -> Result<Vec<RequiredExtension>> {
    let app_dir = crate::installation::absolute(paths, &app.app_dir);
    let runtime_dir = crate::installation::get_runtime(paths, &app.runtime_ref)?
        .map(|runtime| crate::installation::absolute(paths, &runtime.runtime_dir))
        .unwrap_or_else(|| crate::installation::absolute(paths, &app.runtime_dir));
    let mut requirements = Vec::new();

    let runtime_metadata_path = runtime_dir.join("metadata");
    let runtime_metadata = fs::read_to_string(&runtime_metadata_path)
        .with_context(|| format!("read runtime metadata {}", runtime_metadata_path.display()))?;
    requirements.extend(required_from_metadata(
        &runtime_metadata,
        &ExtensionParent::from_ref(&app.runtime_ref)?,
        &app.runtime_origin,
        available_refs,
        facts,
        installed_refs,
    )?);

    let app_metadata_path = app_dir.join("metadata");
    let app_metadata = fs::read_to_string(&app_metadata_path)
        .with_context(|| format!("read app metadata {}", app_metadata_path.display()))?;
    requirements.extend(required_from_metadata(
        &app_metadata,
        &ExtensionParent::from_ref(&app.app_ref)?,
        &app.origin,
        available_refs,
        facts,
        installed_refs,
    )?);
    requirements.sort_by(|left, right| left.ref_name.cmp(&right.ref_name));
    requirements.dedup_by(|left, right| left.ref_name == right.ref_name);
    Ok(requirements)
}

fn required_from_metadata(
    metadata: &str,
    parent: &ExtensionParent,
    preferred_origin: &str,
    available_refs: &BTreeSet<String>,
    facts: &ExtensionFacts,
    installed_refs: &BTreeSet<String>,
) -> Result<Vec<RequiredExtension>> {
    let points = parse_extension_points(metadata);
    let mut requirements = Vec::new();
    for point in &points {
        for ref_name in resolve_extension_refs(std::slice::from_ref(point), parent, available_refs)
        {
            if !installed_refs.contains(&ref_name) && !autodownload_enabled(point, &ref_name, facts)
            {
                continue;
            }
            validate_declaration(point)?;
            requirements.push(RequiredExtension {
                ref_name,
                preferred_origin: preferred_origin.to_string(),
                optional: !point.download_if.is_empty() || point.no_autodownload,
            });
        }
    }
    requirements.sort_by(|left, right| left.ref_name.cmp(&right.ref_name));
    requirements.dedup_by(|left, right| left.ref_name == right.ref_name);
    Ok(requirements)
}

fn autodownload_enabled(point: &ExtensionPoint, ref_name: &str, facts: &ExtensionFacts) -> bool {
    let name = ref_name
        .strip_prefix("runtime/")
        .and_then(|partial| partial.split('/').next())
        .unwrap_or_default();
    if point.name.ends_with(".Debug") {
        return false;
    }
    if point.name.ends_with(".Locale") {
        return true;
    }
    if !point.download_if.is_empty() {
        return facts.matches_any(&point.download_if, name);
    }
    !point.no_autodownload
}

fn validate_declaration(point: &ExtensionPoint) -> Result<()> {
    let Some(directory) = &point.directory else {
        return Ok(());
    };
    if !valid_relative_extension_path(directory) {
        anyhow::bail!("invalid extension directory: {directory:?}");
    }
    if let Some(suffix) = &point.subdirectory_suffix {
        if !valid_relative_extension_path(suffix) {
            anyhow::bail!("invalid extension subdirectory suffix: {suffix:?}");
        }
    }
    for merge_dir in &point.merge_dirs {
        if !valid_relative_extension_path(merge_dir) {
            anyhow::bail!("invalid extension merge directory: {merge_dir:?}");
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/reconciliation.rs"]
mod tests;
