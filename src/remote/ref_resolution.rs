use super::appstream_metadata::fetch_appstream_replacements;
use super::metadata_cache::load_arch_summary;
use super::ostree_summary::{
    lookup_flatpak_metadata, parse_summary_refs, remote_ref_from_summary_info, variant_from_file,
};
use super::{trace_resolution, RemoteApp, RemoteMetadata, RemoteRef, SearchResult};
use crate::paths::Installation;
use crate::runtime::metadata_value;
use crate::storage::{CommitInfo, Storage};
use anyhow::{bail, Context, Result};
use glib::Variant;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

impl RemoteMetadata {
    #[cfg(test)]
    pub(crate) fn empty_for_test(root: &Path) -> Self {
        Self {
            arch: "x86_64".to_string(),
            refs: Vec::new(),
            remote_dir: root.join("remote"),
            summary_path: root.join("summary"),
            collection_id: None,
        }
    }

    pub fn resolve_exact_ref(&self, app_ref: &str) -> Result<RemoteApp> {
        let remote_ref = self
            .refs
            .iter()
            .find(|item| item.name == app_ref)
            .cloned()
            .with_context(|| format!("app ref is no longer present in Flathub: {app_ref}"))?;
        remote_app_from_ref(&self.refs, remote_ref, &self.arch)
    }

    pub fn resolve_app(&self, app_id: &str, replacements: bool) -> Result<RemoteApp> {
        let started = Instant::now();
        let app_ref = self.resolve_app_ref(app_id, replacements)?;
        trace_resolution("select application ref", started);
        remote_app_from_ref(&self.refs, app_ref, &self.arch)
    }

    pub fn collection_id(&self) -> Option<&str> {
        self.collection_id.as_deref()
    }

    pub fn app_history(
        &self,
        paths: &Installation,
        app_id: &str,
    ) -> Result<(RemoteApp, Vec<CommitInfo>)> {
        let remote = self.resolve_app(app_id, true)?;
        let history = self.history_for_remote(paths, &remote)?;
        Ok((remote, history))
    }

    pub fn resolve_app_commit(
        &self,
        paths: &Installation,
        app_ref: &str,
        requested_commit: &str,
    ) -> Result<RemoteApp> {
        self.app_commit(paths, app_ref, requested_commit)
            .map(|(app, _)| app)
    }

    pub fn app_commit(
        &self,
        paths: &Installation,
        app_ref: &str,
        requested_commit: &str,
    ) -> Result<(RemoteApp, CommitInfo)> {
        let tip = self.resolve_exact_ref(app_ref)?;
        let history = self.history_for_remote(paths, &tip)?;
        let commit = select_history_commit(&history, requested_commit)?;
        let metadata = commit.flatpak_metadata.clone().with_context(|| {
            format!(
                "historical commit {} has no Flatpak metadata",
                commit.checksum
            )
        })?;
        let app = remote_app_from_ref(
            &self.refs,
            RemoteRef {
                name: app_ref.to_string(),
                checksum: commit.checksum.clone(),
                metadata: Some(metadata),
                download_size: None,
                installed_size: None,
            },
            &self.arch,
        )?;
        Ok((app, commit.clone()))
    }

    fn history_for_remote(
        &self,
        paths: &Installation,
        remote: &RemoteApp,
    ) -> Result<Vec<CommitInfo>> {
        let summary = fs::read(&self.summary_path)
            .with_context(|| format!("read {}", self.summary_path.display()))?;
        Storage::open(paths)?.commit_history(&summary, &remote.app_ref, &remote.app_commit)
    }

    fn resolve_app_ref(&self, app_id: &str, replacements: bool) -> Result<RemoteRef> {
        if app_id.contains('/') {
            bail!("app id must not contain '/': {app_id}");
        }
        let app_id = if replacements && !app_ref_exists(&self.refs, app_id, &self.arch) {
            resolve_current_app_id(&self.refs, app_id, &self.arch, &self.remote_dir)?
        } else {
            app_id.to_string()
        };
        choose_app_ref(&self.refs, &app_id, &self.arch)
    }
}

pub fn inspect_refs(paths: &Installation, refs: &[String]) -> Result<()> {
    let refs: Vec<String> = if refs.is_empty() {
        vec![
            "app/org.gnome.Calculator/x86_64/stable".to_string(),
            "runtime/org.gnome.Platform/x86_64/50".to_string(),
        ]
    } else {
        refs.to_vec()
    };

    let metadata = load_remote_metadata(paths)?;
    for ref_name in refs {
        let remote_ref = metadata
            .refs
            .iter()
            .find(|candidate| candidate.name == ref_name)
            .with_context(|| format!("ref is not present in Flathub: {ref_name}"))?;
        println!("{ref_name}");
        println!("  commit: {}", remote_ref.checksum);
        if let Some(ref commit_metadata) = remote_ref.metadata {
            if let Some(command) = metadata_value(commit_metadata, "Application", "command") {
                println!("  command: {command}");
            }
            if let Some(runtime) = metadata_value(commit_metadata, "Application", "runtime") {
                println!("  runtime: {runtime}");
            }
        }
    }
    Ok(())
}

pub fn resolve_remote_app(paths: &Installation, app_id: &str) -> Result<RemoteApp> {
    if app_id.contains('/') {
        bail!("app id must not contain '/': {app_id}");
    }
    let total_started = Instant::now();
    let started = Instant::now();
    let (arch, summary_path, collection_id) = load_arch_summary(paths)?;
    trace_resolution("load indexed architecture metadata", started);
    let started = Instant::now();
    if let Some(app) = resolve_exact_app_from_summary(&summary_path, app_id, &arch)? {
        trace_resolution("resolve exact app", started);
        trace_resolution("total exact resolution", total_started);
        return Ok(app);
    }
    trace_resolution("search exact app", started);

    let started = Instant::now();
    let refs = parse_summary_refs(&summary_path)?;
    let metadata = RemoteMetadata {
        arch,
        refs,
        remote_dir: paths.remote_metadata(),
        summary_path,
        collection_id,
    };
    let app = metadata.resolve_app(app_id, true)?;
    trace_resolution("resolve replacement app", started);
    trace_resolution("total replacement resolution", total_started);
    Ok(app)
}

pub fn load_remote_metadata(paths: &Installation) -> Result<RemoteMetadata> {
    let (arch, summary_path, collection_id) = load_arch_summary(paths)?;
    let started = Instant::now();
    let refs = parse_summary_refs(&summary_path)?;
    trace_resolution("parse architecture refs", started);
    Ok(RemoteMetadata {
        arch,
        refs,
        remote_dir: paths.remote_metadata(),
        summary_path,
        collection_id,
    })
}

fn select_history_commit<'a>(history: &'a [CommitInfo], requested: &str) -> Result<&'a CommitInfo> {
    if requested.is_empty() || !requested.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid OSTree commit: {requested:?}");
    }
    let requested = requested.to_ascii_lowercase();
    let mut matches = history
        .iter()
        .filter(|commit| commit.checksum.starts_with(&requested));
    let selected = matches
        .next()
        .with_context(|| format!("commit {requested} is not in the history of this app ref"))?;
    if matches.next().is_some() {
        bail!("commit prefix {requested} is ambiguous");
    }
    Ok(selected)
}

fn resolve_exact_app_from_summary(
    summary_path: &Path,
    app_id: &str,
    arch: &str,
) -> Result<Option<RemoteApp>> {
    let variant = variant_from_file(summary_path, "(a(s(taya{sv}))a{sv})")
        .with_context(|| format!("parse {}", summary_path.display()))?;
    let refs = variant.child_value(0);
    let prefix = format!("app/{app_id}/{arch}/");
    let stable_ref = format!("{prefix}stable");
    let mut candidates = match find_summary_entry(&refs, &stable_ref)? {
        Some(candidate) => vec![candidate],
        None => Vec::new(),
    };

    if candidates.is_empty() {
        for index in 0..refs.n_children() {
            let item = refs.child_value(index);
            let Some(name) = item.child_value(0).str().map(ToOwned::to_owned) else {
                continue;
            };
            if !name.starts_with(&prefix) {
                continue;
            }
            let info = item.child_value(1);
            candidates.push((
                remote_ref_from_summary_info(name, &info)?,
                lookup_flatpak_metadata(&info.child_value(2)),
            ));
        }
    }

    if candidates.is_empty() {
        return Ok(None);
    }
    let candidate_refs = candidates
        .iter()
        .map(|(remote_ref, _)| remote_ref.clone())
        .collect::<Vec<_>>();
    let app_remote_ref = choose_app_ref(&candidate_refs, app_id, arch)?;
    let cached_metadata = candidates
        .into_iter()
        .find(|(remote_ref, _)| remote_ref.name == app_remote_ref.name)
        .and_then(|(_, metadata)| metadata);
    let metadata = match cached_metadata {
        Some(metadata) => {
            trace_resolution("reuse application metadata from summary", Instant::now());
            metadata
        }
        None => bail!(
            "Flathub summary has no xa.data metadata for {}",
            app_remote_ref.name
        ),
    };
    let runtime_ref = metadata_value(&metadata, "Application", "runtime")
        .context("remote app metadata has no Application/runtime")?;
    let runtime_full_ref = format!("runtime/{runtime_ref}");
    let runtime_remote_ref = find_summary_ref(&refs, &runtime_full_ref)?.with_context(|| {
        format!("required runtime ref not found in Flathub summary: {runtime_full_ref}")
    })?;
    remote_app_from_metadata(app_remote_ref, metadata, runtime_remote_ref, arch).map(Some)
}

fn find_summary_ref(refs: &Variant, ref_name: &str) -> Result<Option<RemoteRef>> {
    Ok(find_summary_entry(refs, ref_name)?.map(|(remote_ref, _)| remote_ref))
}

fn find_summary_entry(
    refs: &Variant,
    ref_name: &str,
) -> Result<Option<(RemoteRef, Option<String>)>> {
    let mut left = 0usize;
    let mut right = refs.n_children();
    while left < right {
        let middle = left + (right - left) / 2;
        let item = refs.child_value(middle);
        let name_value = item.child_value(0);
        let name = name_value.str().unwrap_or_default();
        match name.cmp(ref_name) {
            std::cmp::Ordering::Less => left = middle + 1,
            std::cmp::Ordering::Greater => right = middle,
            std::cmp::Ordering::Equal => {
                let info = item.child_value(1);
                return Ok(Some((
                    remote_ref_from_summary_info(name.to_string(), &info)?,
                    lookup_flatpak_metadata(&info.child_value(2)),
                )));
            }
        }
    }

    // OSTree summaries are sorted, but retain a correctness fallback for
    // non-conforming repositories.
    for index in 0..refs.n_children() {
        let item = refs.child_value(index);
        if item.child_value(0).str() != Some(ref_name) {
            continue;
        }
        let info = item.child_value(1);
        return Ok(Some((
            remote_ref_from_summary_info(ref_name.to_string(), &info)?,
            lookup_flatpak_metadata(&info.child_value(2)),
        )));
    }
    Ok(None)
}

fn remote_app_from_ref(
    refs: &[RemoteRef],
    app_remote_ref: RemoteRef,
    arch: &str,
) -> Result<RemoteApp> {
    let app_metadata = app_remote_ref.metadata.clone().with_context(|| {
        format!(
            "Flathub summary has no xa.data metadata for {}",
            app_remote_ref.name
        )
    })?;
    let runtime_ref = metadata_value(&app_metadata, "Application", "runtime")
        .context("remote app metadata has no Application/runtime")?;
    let runtime_full_ref = format!("runtime/{runtime_ref}");
    let started = Instant::now();
    let runtime_remote_ref = refs
        .iter()
        .find(|remote_ref| remote_ref.name == runtime_full_ref)
        .cloned()
        .with_context(|| {
            format!("required runtime ref not found in Flathub summary: {runtime_full_ref}")
        })?;
    trace_resolution("select runtime ref", started);
    remote_app_from_metadata(app_remote_ref, app_metadata, runtime_remote_ref, arch)
}

fn remote_app_from_metadata(
    app_remote_ref: RemoteRef,
    metadata: String,
    runtime_remote_ref: RemoteRef,
    arch: &str,
) -> Result<RemoteApp> {
    let app_ref_parts = split_flatpak_ref(&app_remote_ref.name)?;
    let app_id = app_ref_parts.name;
    let metadata_app_id = metadata_value(&metadata, "Application", "name")
        .context("remote app metadata has no Application/name")?;
    if metadata_app_id != app_id {
        bail!("remote metadata app id mismatch: requested {app_id}, found {metadata_app_id}");
    }

    let runtime_ref = metadata_value(&metadata, "Application", "runtime")
        .context("remote app metadata has no Application/runtime")?;
    let sdk_ref = metadata_value(&metadata, "Application", "sdk");
    let command = metadata_value(&metadata, "Application", "command")
        .context("remote app metadata has no Application/command")?;
    if command.split_whitespace().count() != 1 {
        bail!("entry command must be a single executable for this POC: {command:?}");
    }

    Ok(RemoteApp {
        app_id,
        app_ref: app_remote_ref.name,
        app_commit: app_remote_ref.checksum,
        arch: arch.to_string(),
        branch: app_ref_parts.branch,
        runtime_ref,
        runtime_commit: runtime_remote_ref.checksum,
        sdk_ref,
        download_size: app_remote_ref.download_size,
        installed_size: app_remote_ref.installed_size,
        command,
    })
}

pub fn search_apps(paths: &Installation, query: &str) -> Result<Vec<SearchResult>> {
    let query = query.to_ascii_lowercase();
    let metadata = load_remote_metadata(paths)?;
    let arch = metadata.arch;
    let mut results = Vec::new();

    for remote_ref in metadata.refs {
        let Ok(parts) = split_flatpak_ref(&remote_ref.name) else {
            continue;
        };
        if parts.kind != "app" || parts.arch != arch {
            continue;
        }
        if !parts.name.to_ascii_lowercase().contains(&query) {
            continue;
        }
        results.push(SearchResult {
            app_id: parts.name,
            app_ref: remote_ref.name,
            arch: parts.arch,
            branch: parts.branch,
        });
    }

    results.sort_by(|left, right| {
        left.app_id
            .cmp(&right.app_id)
            .then_with(|| left.branch.cmp(&right.branch))
    });
    Ok(results)
}

fn resolve_current_app_id(
    refs: &[RemoteRef],
    requested: &str,
    arch: &str,
    remote_dir: &Path,
) -> Result<String> {
    let replacements = fetch_appstream_replacements(remote_dir, arch)?;
    resolve_current_app_id_from_replacements(refs, &replacements, requested, arch)
}

fn resolve_current_app_id_from_replacements(
    refs: &[RemoteRef],
    replacements: &BTreeMap<String, Vec<String>>,
    requested: &str,
    arch: &str,
) -> Result<String> {
    let mut current = requested.to_string();
    let mut seen = BTreeSet::new();

    while seen.insert(current.clone()) {
        let Some(candidates) = replacements.get(&current) else {
            return Ok(current);
        };
        let mut available = candidates
            .iter()
            .filter(|candidate| app_ref_exists(refs, candidate, arch))
            .cloned()
            .collect::<Vec<_>>();
        available.sort();
        available.dedup();

        match available.len() {
            0 => return Ok(current),
            1 => {
                let replacement = available.remove(0);
                eprintln!("info: Flathub app id {current} is replaced by {replacement}");
                current = replacement;
            }
            _ => bail!(
                "multiple Flathub replacements found for {current} on {arch}: {}",
                available.join(", ")
            ),
        }
    }

    bail!("cycle in Flathub replacement metadata for app id {requested}");
}

fn app_ref_exists(refs: &[RemoteRef], app_id: &str, arch: &str) -> bool {
    refs.iter().any(|remote_ref| {
        let Ok(parts) = split_flatpak_ref(&remote_ref.name) else {
            return false;
        };
        parts.kind == "app" && parts.name == app_id && parts.arch == arch
    })
}

fn choose_app_ref(refs: &[RemoteRef], app_id: &str, arch: &str) -> Result<RemoteRef> {
    let mut candidates: Vec<(FlatpakRefParts, RemoteRef)> = refs
        .iter()
        .filter_map(|remote_ref| {
            let parts = split_flatpak_ref(&remote_ref.name).ok()?;
            if parts.kind == "app" && parts.name == app_id && parts.arch == arch {
                Some((parts, remote_ref.clone()))
            } else {
                None
            }
        })
        .collect();
    candidates.sort_by(|left, right| left.0.branch.cmp(&right.0.branch));

    if let Some((_, remote_ref)) = candidates
        .iter()
        .find(|(parts, _)| parts.branch == "stable")
    {
        return Ok(remote_ref.clone());
    }

    if candidates.len() == 1 {
        return Ok(candidates[0].1.clone());
    }

    if candidates.is_empty() {
        bail!("no Flathub ref found for app id {app_id} on architecture {arch}");
    }

    let branches = candidates
        .iter()
        .map(|(parts, _)| parts.branch.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    bail!("multiple Flathub branches found for {app_id} on {arch}, and none is stable: {branches}");
}

#[derive(Debug, Clone)]
struct FlatpakRefParts {
    kind: String,
    name: String,
    arch: String,
    branch: String,
}

fn split_flatpak_ref(ref_name: &str) -> Result<FlatpakRefParts> {
    let mut parts = ref_name.splitn(4, '/');
    let kind = parts.next().context("missing ref kind")?;
    let name = parts.next().context("missing ref name")?;
    let arch = parts.next().context("missing ref arch")?;
    let branch = parts.next().context("missing ref branch")?;
    Ok(FlatpakRefParts {
        kind: kind.to_string(),
        name: name.to_string(),
        arch: arch.to_string(),
        branch: branch.to_string(),
    })
}

pub(super) fn host_flatpak_arch() -> Result<String> {
    let output = Command::new("uname")
        .arg("-m")
        .output()
        .context("determine host architecture")?;
    if !output.status.success() {
        bail!("uname -m failed with status {}", output.status);
    }
    let machine = String::from_utf8(output.stdout)?.trim().to_string();
    match machine.as_str() {
        "amd64" | "x86_64" => Ok("x86_64".to_string()),
        "aarch64" | "arm64" => Ok("aarch64".to_string()),
        _ => bail!("unsupported host architecture for Flatpak POC: {machine}"),
    }
}

pub fn checkout_ref(paths: &Installation, ref_name: &str, dest: PathBuf) -> Result<()> {
    let (_, summary_path, _) = load_arch_summary(paths)?;
    let refs = parse_summary_refs(&summary_path)?;
    let checksum = refs
        .iter()
        .find(|candidate| candidate.name == ref_name)
        .map(|candidate| candidate.checksum.as_str())
        .with_context(|| format!("ref is not present in Flathub: {ref_name}"))?;
    let summary =
        fs::read(&summary_path).with_context(|| format!("read {}", summary_path.display()))?;
    Storage::open(paths)?.checkout(&summary, ref_name, checksum, &dest)
}

#[cfg(test)]
#[path = "tests/ref_resolution.rs"]
mod tests;
