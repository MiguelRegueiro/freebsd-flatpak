use super::appstream_metadata::fetch_appstream_replacements;
use super::metadata_cache::load_arch_summary;
use super::ostree_summary::parse_summary_refs;
use super::{
    trace_resolution, Remote, RemoteApp, RemoteMetadata, RemoteRef, RemoteRefInfo, RemoteRuntime,
    ResolvedRemoteRef, SearchResult,
};
use crate::architecture::FlatpakArchitecture;
use crate::flatpak_ref::{FlatpakRef, PartialRef, RefKind};
use crate::installation::installation_paths::Installation;
use crate::installation::metadata_value;
use crate::ostree::{CommitInfo, Storage};
use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

impl RemoteMetadata {
    #[cfg(test)]
    pub(crate) fn empty_for_test(root: &Path) -> Self {
        Self {
            remote: Remote {
                name: "flathub".to_string(),
                url: "https://example.invalid/repo".to_string(),
                title: None,
                enabled: true,
                gpg_verify: false,
                gpg_key: None,
            },
            arch: "x86_64".to_string(),
            refs: Vec::new(),
            remote_dir: root.join("remote"),
            summary_path: root.join("summary"),
            collection_id: None,
        }
    }

    pub fn resolve_runtime_commit(
        &self,
        paths: &Installation,
        runtime_ref: &str,
        requested_commit: &str,
    ) -> Result<RemoteRuntime> {
        let tip = self.resolve_exact_runtime(runtime_ref)?;
        let history = self.history_for_ref(paths, runtime_ref, &tip.runtime_commit)?;
        let commit = select_history_commit(&history, requested_commit)?;
        remote_runtime_from_ref(
            RemoteRef {
                name: runtime_ref.to_string(),
                checksum: commit.checksum.clone(),
                metadata: commit.flatpak_metadata.clone(),
                download_size: None,
                installed_size: None,
            },
            &self.remote.name,
        )
    }

    pub fn resolve_runtime(&self, requested: &str) -> Result<RemoteRuntime> {
        let partial = PartialRef::parse(requested)?;
        partial.effective_kind(Some(RefKind::Runtime))?;
        let remote_ref = select_ref(&self.refs, &partial, RefKind::Runtime, &self.arch)?;
        remote_runtime_from_ref(remote_ref, &self.remote.name)
    }

    pub fn resolve_exact_runtime(&self, runtime_ref: &str) -> Result<RemoteRuntime> {
        let remote_ref = self
            .refs
            .iter()
            .find(|item| item.name == runtime_ref)
            .cloned()
            .with_context(|| {
                format!(
                    "runtime ref is no longer present in {}: {runtime_ref}",
                    self.remote.name
                )
            })?;
        remote_runtime_from_ref(remote_ref, &self.remote.name)
    }

    pub fn resolve_exact_ref(&self, app_ref: &str) -> Result<RemoteApp> {
        let remote_ref = self
            .refs
            .iter()
            .find(|item| item.name == app_ref)
            .cloned()
            .with_context(|| {
                format!(
                    "app ref is no longer present in {}: {app_ref}",
                    self.remote.name
                )
            })?;
        remote_app_from_ref(&self.refs, remote_ref, &self.arch, &self.remote.name)
    }

    pub fn resolve_exact_ref_with_runtime(
        &self,
        paths: &Installation,
        app_ref: &str,
    ) -> Result<RemoteApp> {
        let app_ref = self
            .refs
            .iter()
            .find(|item| item.name == app_ref)
            .cloned()
            .with_context(|| {
                format!(
                    "app ref is no longer present in {}: {app_ref}",
                    self.remote.name
                )
            })?;
        resolve_ref_with_runtime_fallback(paths, self, app_ref)
    }

    #[cfg(test)]
    pub fn resolve_app(&self, app_id: &str, replacements: bool) -> Result<RemoteApp> {
        let started = Instant::now();
        let app_ref = self.resolve_app_ref(app_id, replacements)?;
        trace_resolution("select application ref", started);
        remote_app_from_ref(&self.refs, app_ref, &self.arch, &self.remote.name)
    }

    pub fn collection_id(&self) -> Option<&str> {
        self.collection_id.as_deref()
    }

    pub(crate) fn remote_name(&self) -> &str {
        &self.remote.name
    }

    pub(crate) fn ref_checksum(&self, ref_name: &str) -> Option<&str> {
        self.refs
            .iter()
            .find(|item| item.name == ref_name)
            .map(|item| item.checksum.as_str())
    }

    pub(crate) fn summary_bytes(&self) -> Result<Vec<u8>> {
        fs::read(&self.summary_path)
            .with_context(|| format!("read {}", self.summary_path.display()))
    }

    pub fn list_refs(&self) -> Vec<RemoteRefInfo> {
        let mut refs = self
            .refs
            .iter()
            .filter_map(|item| {
                let parts = split_flatpak_ref(&item.name).ok()?;
                (parts.arch == self.arch).then(|| RemoteRefInfo {
                    remote: self.remote.name.clone(),
                    ref_name: item.name.clone(),
                    arch: parts.arch,
                    branch: parts.branch,
                })
            })
            .collect::<Vec<_>>();
        refs.sort_by(|left, right| left.ref_name.cmp(&right.ref_name));
        refs
    }

    pub fn app_history(
        &self,
        paths: &Installation,
        app_id: &str,
    ) -> Result<(RemoteApp, Vec<CommitInfo>)> {
        let remote = resolve_app_with_runtime_fallback(paths, self, app_id)?;
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
        let history = self.history_for_ref(paths, app_ref, &tip.app_commit)?;
        let commit = select_history_commit(&history, requested_commit)?;
        let metadata = commit.flatpak_metadata.clone().with_context(|| {
            format!(
                "historical commit {} has no Flatpak metadata",
                commit.checksum
            )
        })?;
        let app = resolve_ref_with_runtime_fallback(
            paths,
            self,
            RemoteRef {
                name: app_ref.to_string(),
                checksum: commit.checksum.clone(),
                metadata: Some(metadata),
                download_size: None,
                installed_size: None,
            },
        )?;
        Ok((app, commit.clone()))
    }

    fn history_for_remote(
        &self,
        paths: &Installation,
        remote: &RemoteApp,
    ) -> Result<Vec<CommitInfo>> {
        self.history_for_ref(paths, &remote.app_ref, &remote.app_commit)
    }

    fn history_for_ref(
        &self,
        paths: &Installation,
        ref_name: &str,
        commit: &str,
    ) -> Result<Vec<CommitInfo>> {
        let summary = fs::read(&self.summary_path)
            .with_context(|| format!("read {}", self.summary_path.display()))?;
        Storage::open(paths)?.commit_history(
            &self.remote.name,
            &summary,
            ref_name,
            commit,
            self.remote.gpg_verify,
        )
    }

    fn resolve_app_ref(&self, requested: &str, replacements: bool) -> Result<RemoteRef> {
        let mut partial = PartialRef::parse(requested)?;
        partial.effective_kind(Some(RefKind::App))?;
        let arch = partial.arch.as_deref().unwrap_or(&self.arch);
        if replacements && !app_ref_exists(&self.refs, &partial.id, arch) {
            partial.id = resolve_current_app_id(
                &self.refs,
                &partial.id,
                arch,
                &self.remote,
                &self.remote_dir,
            )?;
        }
        select_ref(&self.refs, &partial, RefKind::App, &self.arch)
    }
}

pub fn inspect_refs(paths: &Installation, refs: &[String]) -> Result<()> {
    let refs: Vec<String> = if refs.is_empty() {
        let arch = host_flatpak_arch()?;
        vec![
            format!("app/org.gnome.Calculator/{arch}/stable"),
            format!("runtime/org.gnome.Platform/{arch}/50"),
        ]
    } else {
        refs.to_vec()
    };

    let metadata = load_remote_metadata(paths, super::DEFAULT_REMOTE)?;
    for ref_name in refs {
        let remote_ref = metadata
            .refs
            .iter()
            .find(|candidate| candidate.name == ref_name)
            .with_context(|| {
                format!("ref is not present in {}: {ref_name}", metadata.remote.name)
            })?;
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

pub fn resolve_remote_ref(
    paths: &Installation,
    remote_name: Option<&str>,
    requested: &str,
    kind: Option<RefKind>,
) -> Result<ResolvedRemoteRef> {
    let partial = PartialRef::parse(requested)?;
    let requested_kind = partial.effective_kind(kind)?;

    let candidates = candidate_remotes(paths, remote_name)?;
    let mut failures = Vec::new();
    for remote in candidates {
        let metadata = match load_remote_metadata_for(paths, remote.clone()) {
            Ok(metadata) => metadata,
            Err(error) => {
                failures.push(format!("{}: {error:#}", remote.name));
                continue;
            }
        };
        let resolved = match requested_kind {
            Some(RefKind::App) => resolve_app_with_runtime_fallback(paths, &metadata, requested)
                .map(ResolvedRemoteRef::App),
            Some(RefKind::Runtime) => metadata
                .resolve_runtime(requested)
                .map(ResolvedRemoteRef::Runtime),
            None => {
                let app = resolve_app_with_runtime_fallback(paths, &metadata, requested);
                let runtime = metadata.resolve_runtime(requested);
                match (app, runtime) {
                    (Ok(app), Err(_)) => Ok(ResolvedRemoteRef::App(app)),
                    (Err(_), Ok(runtime)) => Ok(ResolvedRemoteRef::Runtime(runtime)),
                    (Ok(_), Ok(_)) => bail!(
                        "{requested} matches both an application and a runtime; use --app or --runtime"
                    ),
                    (Err(app_error), Err(runtime_error)) => bail!(
                        "application: {app_error:#}; runtime: {runtime_error:#}"
                    ),
                }
            }
        };
        match resolved {
            Ok(resolved) => return Ok(resolved),
            Err(error) => failures.push(format!("{}: {error:#}", remote.name)),
        }
    }
    bail!(
        "ref {requested} was not found in an enabled remote ({})",
        failures.join("; ")
    )
}

fn candidate_remotes(paths: &Installation, remote_name: Option<&str>) -> Result<Vec<Remote>> {
    let candidates = if let Some(name) = remote_name {
        let remote = super::get_remote(paths, name)?;
        if !remote.enabled {
            bail!("remote is disabled: {name}");
        }
        vec![remote]
    } else {
        let mut remotes = super::enabled_remotes(paths)?;
        remotes.sort_by_key(|remote| (remote.name != super::DEFAULT_REMOTE, remote.name.clone()));
        remotes
    };
    if candidates.is_empty() {
        bail!("no enabled remotes are configured");
    }
    Ok(candidates)
}

pub fn resolve_remote_app(
    paths: &Installation,
    remote_name: Option<&str>,
    app_id: &str,
) -> Result<RemoteApp> {
    let candidates = if let Some(name) = remote_name {
        let remote = super::get_remote(paths, name)?;
        if !remote.enabled {
            bail!("remote is disabled: {name}");
        }
        vec![remote]
    } else {
        let mut remotes = super::enabled_remotes(paths)?;
        remotes.sort_by_key(|remote| (remote.name != super::DEFAULT_REMOTE, remote.name.clone()));
        remotes
    };
    if candidates.is_empty() {
        bail!("no enabled remotes are configured");
    }
    let mut failures = Vec::new();
    for remote in candidates {
        let metadata = match load_remote_metadata_for(paths, remote.clone()) {
            Ok(metadata) => metadata,
            Err(error) => {
                failures.push(format!("{}: {error:#}", remote.name));
                continue;
            }
        };
        match resolve_app_with_runtime_fallback(paths, &metadata, app_id) {
            Ok(app) => return Ok(app),
            Err(error) => failures.push(format!("{}: {error:#}", remote.name)),
        }
    }
    bail!(
        "ref {app_id} was not found in an enabled remote ({})",
        failures.join("; ")
    )
}

fn resolve_app_with_runtime_fallback(
    paths: &Installation,
    metadata: &RemoteMetadata,
    app_id: &str,
) -> Result<RemoteApp> {
    let app_ref = metadata.resolve_app_ref(app_id, true)?;
    resolve_ref_with_runtime_fallback(paths, metadata, app_ref)
}

fn resolve_ref_with_runtime_fallback(
    paths: &Installation,
    metadata: &RemoteMetadata,
    app_ref: RemoteRef,
) -> Result<RemoteApp> {
    let app_metadata = app_ref.metadata.clone().with_context(|| {
        format!(
            "{} summary has no Flatpak metadata for {}",
            metadata.remote.name, app_ref.name
        )
    })?;
    let runtime_ref = metadata_value(&app_metadata, "Application", "runtime")
        .context("remote app metadata has no Application/runtime")?;
    let runtime_full_ref = format!("runtime/{runtime_ref}");
    if let Some(installed) = crate::installation::get_runtime(paths, &runtime_ref)? {
        let runtime = if installed.origin == metadata.remote.name {
            metadata
                .refs
                .iter()
                .find(|item| item.name == runtime_full_ref)
                .cloned()
        } else {
            load_remote_metadata(paths, &installed.origin)
                .ok()
                .and_then(|installed_metadata| {
                    installed_metadata
                        .refs
                        .iter()
                        .find(|item| item.name == runtime_full_ref)
                        .cloned()
                })
        }
        .unwrap_or(RemoteRef {
            name: runtime_full_ref.clone(),
            checksum: installed.runtime_commit,
            metadata: None,
            download_size: None,
            installed_size: Some(installed.installed_size),
        });
        return remote_app_from_metadata(
            app_ref,
            app_metadata,
            runtime,
            &metadata.arch,
            &metadata.remote.name,
            &installed.origin,
        );
    }
    if let Some(runtime) = metadata
        .refs
        .iter()
        .find(|item| item.name == runtime_full_ref)
        .cloned()
    {
        return remote_app_from_metadata(
            app_ref,
            app_metadata,
            runtime,
            &metadata.arch,
            &metadata.remote.name,
            &metadata.remote.name,
        );
    }
    for remote in super::enabled_remotes(paths)? {
        if remote.name == metadata.remote.name {
            continue;
        }
        let candidate = match load_remote_metadata_for(paths, remote.clone()) {
            Ok(candidate) => candidate,
            Err(error) => {
                eprintln!(
                    "warning: cannot inspect remote {} for runtime {}: {error:#}",
                    remote.name, runtime_ref
                );
                continue;
            }
        };
        if let Some(runtime) = candidate
            .refs
            .iter()
            .find(|item| item.name == runtime_full_ref)
            .cloned()
        {
            return remote_app_from_metadata(
                app_ref,
                app_metadata,
                runtime,
                &metadata.arch,
                &metadata.remote.name,
                &remote.name,
            );
        }
    }
    bail!("required runtime ref not found in an enabled remote: {runtime_full_ref}")
}

pub fn load_remote_metadata(paths: &Installation, name: &str) -> Result<RemoteMetadata> {
    let remote = super::get_remote(paths, name)?;
    if !remote.enabled {
        bail!("remote is disabled: {name}");
    }
    load_remote_metadata_for(paths, remote)
}

fn load_remote_metadata_for(paths: &Installation, remote: Remote) -> Result<RemoteMetadata> {
    let (arch, summary_path, collection_id) = load_arch_summary(paths, &remote)?;
    let started = Instant::now();
    let refs = parse_summary_refs(&summary_path)?;
    trace_resolution("parse architecture refs", started);
    Ok(RemoteMetadata {
        remote: remote.clone(),
        arch,
        refs,
        remote_dir: paths.remote_metadata(&remote.name),
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

fn remote_app_from_ref(
    refs: &[RemoteRef],
    app_remote_ref: RemoteRef,
    arch: &str,
    origin: &str,
) -> Result<RemoteApp> {
    let app_metadata = app_remote_ref.metadata.clone().with_context(|| {
        format!(
            "remote summary has no Flatpak metadata for {}",
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
            format!("required runtime ref not found in remote summary: {runtime_full_ref}")
        })?;
    trace_resolution("select runtime ref", started);
    remote_app_from_metadata(
        app_remote_ref,
        app_metadata,
        runtime_remote_ref,
        arch,
        origin,
        origin,
    )
}

fn remote_app_from_metadata(
    app_remote_ref: RemoteRef,
    metadata: String,
    runtime_remote_ref: RemoteRef,
    arch: &str,
    origin: &str,
    runtime_origin: &str,
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
        bail!("entry command must be a single executable: {command:?}");
    }

    Ok(RemoteApp {
        origin: origin.to_string(),
        runtime_origin: runtime_origin.to_string(),
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
    let mut results = Vec::new();
    let mut loaded = 0usize;
    let mut failures = Vec::new();
    let configured_remotes = super::enabled_remotes(paths)?;
    if configured_remotes.is_empty() {
        bail!("no enabled remotes are configured");
    }
    for configured in configured_remotes {
        let metadata = match load_remote_metadata_for(paths, configured.clone()) {
            Ok(metadata) => metadata,
            Err(error) => {
                failures.push(format!("{}: {error:#}", configured.name));
                continue;
            }
        };
        loaded += 1;
        let arch = metadata.arch;
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
                remote: configured.name.clone(),
                app_id: parts.name,
                app_ref: remote_ref.name,
                arch: parts.arch,
                branch: parts.branch,
            });
        }
    }

    if loaded == 0 && !failures.is_empty() {
        bail!("failed to load configured remotes: {}", failures.join("; "));
    }
    for failure in failures {
        eprintln!("warning: search skipped remote {failure}");
    }

    results.sort_by(|left, right| {
        left.app_id
            .cmp(&right.app_id)
            .then_with(|| left.branch.cmp(&right.branch))
            .then_with(|| left.remote.cmp(&right.remote))
    });
    Ok(results)
}

fn resolve_current_app_id(
    refs: &[RemoteRef],
    requested: &str,
    arch: &str,
    remote: &Remote,
    remote_dir: &Path,
) -> Result<String> {
    let replacements = fetch_appstream_replacements(remote, remote_dir, arch)?;
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
                eprintln!("info: app id {current} is replaced by {replacement}");
                current = replacement;
            }
            _ => bail!(
                "multiple replacements found for {current} on {arch}: {}",
                available.join(", ")
            ),
        }
    }

    bail!("cycle in replacement metadata for app id {requested}");
}

fn app_ref_exists(refs: &[RemoteRef], app_id: &str, arch: &str) -> bool {
    refs.iter().any(|remote_ref| {
        let Ok(parts) = split_flatpak_ref(&remote_ref.name) else {
            return false;
        };
        parts.kind == "app" && parts.name == app_id && parts.arch == arch
    })
}

fn select_ref(
    refs: &[RemoteRef],
    partial: &PartialRef,
    kind: RefKind,
    default_arch: &str,
) -> Result<RemoteRef> {
    let arch = partial.arch.as_deref().unwrap_or(default_arch);
    let mut candidates = refs
        .iter()
        .filter_map(|candidate| {
            let parsed = FlatpakRef::parse(&candidate.name).ok()?;
            (parsed.kind == kind
                && parsed.id == partial.id
                && parsed.arch == arch
                && partial
                    .branch
                    .as_deref()
                    .is_none_or(|branch| parsed.branch == branch))
            .then_some((parsed, candidate.clone()))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|candidate| candidate.0.full_ref());
    if partial.branch.is_none() {
        if let Some((_, candidate)) = candidates
            .iter()
            .find(|(parsed, _)| parsed.branch == "stable")
        {
            return Ok(candidate.clone());
        }
    }
    match candidates.len() {
        0 => bail!(
            "no remote {} ref matches {}/{}/{}",
            kind.as_str(),
            partial.id,
            arch,
            partial.branch.as_deref().unwrap_or("*")
        ),
        1 => Ok(candidates.remove(0).1),
        _ => bail!(
            "multiple remote refs match {}: {}",
            partial.id,
            candidates
                .iter()
                .map(|(parsed, _)| parsed.full_ref())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn remote_runtime_from_ref(remote_ref: RemoteRef, origin: &str) -> Result<RemoteRuntime> {
    let parsed = FlatpakRef::parse(&remote_ref.name)?;
    if parsed.kind != RefKind::Runtime {
        bail!("{} is not a runtime ref", remote_ref.name);
    }
    debug_assert_eq!(parsed.full_ref(), remote_ref.name);
    let runtime_ref = parsed.partial_ref();
    Ok(RemoteRuntime {
        origin: origin.to_string(),
        runtime_id: parsed.id,
        runtime_ref,
        runtime_commit: remote_ref.checksum,
        arch: parsed.arch,
        branch: parsed.branch,
    })
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
    Ok(FlatpakArchitecture::host()?.flatpak_name().to_string())
}

pub fn checkout_ref(paths: &Installation, ref_name: &str, dest: PathBuf) -> Result<()> {
    let remote = super::get_remote(paths, super::DEFAULT_REMOTE)?;
    let (_, summary_path, _) = load_arch_summary(paths, &remote)?;
    let refs = parse_summary_refs(&summary_path)?;
    let checksum = refs
        .iter()
        .find(|candidate| candidate.name == ref_name)
        .map(|candidate| candidate.checksum.as_str())
        .with_context(|| format!("ref is not present in {}: {ref_name}", remote.name))?;
    let summary =
        fs::read(&summary_path).with_context(|| format!("read {}", summary_path.display()))?;
    Storage::open(paths)?
        .deploy(
            &summary,
            &[crate::ostree::Deployment {
                remote: &remote.name,
                kind: "ref",
                ref_name,
                checksum,
                destination: &dest,
                force: true,
            }],
        )
        .map(|_| ())
}

#[cfg(test)]
#[path = "tests/ref_resolution.rs"]
mod tests;
