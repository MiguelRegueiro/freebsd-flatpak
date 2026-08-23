use crate::paths::Installation;
use crate::storage::{CommitInfo, Deployment, Storage};
use anyhow::{bail, Context, Result};
use glib::{Bytes, Checksum, ChecksumType, Variant, VariantTy};
use miniz_oxide::inflate::decompress_to_vec;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const REMOTE: &str = "https://dl.flathub.org/repo";

#[derive(Debug, Clone)]
pub struct FlatpakApp {
    pub app_id: String,
    pub app_dir: PathBuf,
    pub runtime_ref: String,
    pub runtime_dir: PathBuf,
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Debug, Default)]
pub struct ResolveAppOptions {
    pub app_dir: Option<PathBuf>,
    pub runtime_dir: Option<PathBuf>,
    pub entry: Option<String>,
    pub args: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct InstalledApp {
    pub app_id: String,
    pub app_ref: String,
    pub app_commit: String,
    pub app_dir: PathBuf,
    pub arch: String,
    pub branch: String,
    pub runtime_ref: String,
    pub runtime_commit: String,
    pub runtime_dir: PathBuf,
    pub command: String,
    pub timings: InstallTimings,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct InstallTimings {
    pub resolution: Duration,
    pub pull: Duration,
    pub checkout: Duration,
}

#[derive(Debug, Clone)]
pub struct RuntimeGlExtension {
    pub ref_name: String,
    pub checkout_dir: PathBuf,
    pub runtime_mount_relative: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RuntimeVaapiExtension {
    pub ref_name: String,
    pub checkout_dir: PathBuf,
    pub runtime_mount_relative: PathBuf,
    pub ld_library_relative: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct AppExtension {
    pub name: String,
    pub ref_name: String,
    pub checkout_dir: PathBuf,
    pub app_mount_relative: PathBuf,
    pub ld_library_relative: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub app_id: String,
    pub app_ref: String,
    pub arch: String,
    pub branch: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppstreamInfo {
    pub name: Option<String>,
    pub summary: Option<String>,
    pub version: Option<String>,
    pub license: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RemoteApp {
    pub app_id: String,
    pub app_ref: String,
    pub app_commit: String,
    pub arch: String,
    pub branch: String,
    pub runtime_ref: String,
    pub runtime_commit: String,
    pub sdk_ref: Option<String>,
    pub download_size: Option<u64>,
    pub installed_size: Option<u64>,
    pub command: String,
}

#[derive(Debug, Clone)]
struct RemoteRef {
    name: String,
    checksum: String,
    metadata: Option<String>,
    download_size: Option<u64>,
    installed_size: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct RemoteMetadata {
    arch: String,
    refs: Vec<RemoteRef>,
    remote_dir: PathBuf,
    summary_path: PathBuf,
    collection_id: Option<String>,
}

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

    pub fn appstream_info(&self, app_id: &str) -> Result<Option<AppstreamInfo>> {
        let path = fetch_appstream(&self.remote_dir, &self.arch)?;
        let compressed = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let xml = decompress_appstream_xml(&compressed)
            .with_context(|| format!("decompress {}", path.display()))?;
        Ok(parse_appstream_info(&xml, app_id))
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

impl RuntimeGlExtension {
    pub fn ref_name(&self) -> &str {
        &self.ref_name
    }
}

impl RuntimeVaapiExtension {
    pub fn ref_name(&self) -> &str {
        &self.ref_name
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

pub fn repair_repo(paths: &Installation) -> Result<usize> {
    Storage::open(paths)?.fsck_all()
}

pub fn recover_storage(paths: &Installation) -> Result<()> {
    drop(Storage::open(paths)?);
    Ok(())
}

pub fn prune_repo(paths: &Installation) -> Result<(i32, i32, u64)> {
    Storage::open(paths)?.prune()
}

pub fn remove_repo_refs(paths: &Installation, refs: &[&str]) -> Result<()> {
    Storage::open(paths)?.remove_refs(refs)
}

pub fn update_app(
    paths: &Installation,
    remote: &RemoteApp,
    force_app: bool,
    force_runtime: bool,
) -> Result<InstalledApp> {
    checkout_remote_app(paths, remote, force_app, force_runtime)
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

fn load_arch_summary(paths: &Installation) -> Result<(String, PathBuf, Option<String>)> {
    let started = Instant::now();
    let arch = host_flatpak_arch()?;
    trace_resolution("detect architecture", started);
    let started = Instant::now();
    let index_path = fetch_summary_index(paths)?;
    trace_resolution("fetch summary index", started);
    let started = Instant::now();
    let (digest, collection_id) = parse_summary_index(&index_path, &arch)?;
    if std::env::var_os("FREEBSD_FLATPAK_TRACE_RESOLUTION").is_some() {
        eprintln!("resolution metadata: {arch} summary {digest}");
    }
    trace_resolution("parse summary index", started);
    let started = Instant::now();
    let summary_path = fetch_subsummary(paths, &arch, &digest)?;
    trace_resolution("fetch architecture summary", started);
    let started = Instant::now();
    let summary_path = cache_uncompressed_subsummary(paths, &digest, &summary_path)?;
    trace_resolution("prepare architecture summary cache", started);
    Ok((arch, summary_path, collection_id))
}

fn trace_resolution(label: &str, started: Instant) {
    if std::env::var_os("FREEBSD_FLATPAK_TRACE_RESOLUTION").is_some() {
        eprintln!(
            "resolution timing: {label}: {:.3}s",
            started.elapsed().as_secs_f64()
        );
    }
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

fn checkout_remote_app(
    paths: &Installation,
    remote: &RemoteApp,
    force_app: bool,
    force_runtime: bool,
) -> Result<InstalledApp> {
    let app_dir =
        generation_checkout_dir(&paths.app(&remote.app_id), &remote.app_commit, force_app);
    let runtime_dir = generation_checkout_dir(
        &paths
            .runtimes()
            .join(runtime_checkout_dir(&remote.runtime_ref)),
        &remote.runtime_commit,
        force_runtime,
    );
    let (_, summary_path, _) = load_arch_summary(paths)?;
    let summary =
        fs::read(&summary_path).with_context(|| format!("read {}", summary_path.display()))?;
    let runtime_full_ref = format!("runtime/{}", remote.runtime_ref);
    let storage = Storage::open(paths)?;
    let mut timings = storage.deploy(
        &summary,
        &[
            Deployment {
                kind: "application",
                ref_name: &remote.app_ref,
                checksum: &remote.app_commit,
                destination: &app_dir,
                force: force_app,
            },
            Deployment {
                kind: "runtime",
                ref_name: &runtime_full_ref,
                checksum: &remote.runtime_commit,
                destination: &runtime_dir,
                force: force_runtime,
            },
        ],
    )?;
    drop(storage);
    let (_, extension_timings) =
        ensure_default_gl_extension_timed(paths, &remote.runtime_ref, &runtime_dir)?;
    timings.pull += extension_timings.pull;
    timings.checkout += extension_timings.checkout;

    Ok(InstalledApp {
        app_id: remote.app_id.clone(),
        app_ref: remote.app_ref.clone(),
        app_commit: remote.app_commit.clone(),
        app_dir,
        arch: remote.arch.clone(),
        branch: remote.branch.clone(),
        runtime_ref: remote.runtime_ref.clone(),
        runtime_commit: remote.runtime_commit.clone(),
        runtime_dir,
        command: remote.command.clone(),
        timings: InstallTimings {
            resolution: Duration::ZERO,
            pull: timings.pull,
            checkout: timings.checkout,
        },
    })
}

fn generation_checkout_dir(base: &Path, commit: &str, force: bool) -> PathBuf {
    let ordinary = base.join(commit);
    if !force || !ordinary.exists() {
        return ordinary;
    }
    // A forced repair must not replace a checkout which a sandbox may have
    // pinned.  The app state record will atomically select this repaired copy.
    for sequence in 0u64.. {
        let candidate = base.join(format!("{commit}.repair-{}-{sequence}", std::process::id()));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

pub fn ensure_default_gl_extension(
    paths: &Installation,
    runtime_ref: &str,
    runtime_dir: &Path,
) -> Result<Option<RuntimeGlExtension>> {
    Ok(ensure_default_gl_extension_timed(paths, runtime_ref, runtime_dir)?.0)
}

fn ensure_default_gl_extension_timed(
    paths: &Installation,
    runtime_ref: &str,
    runtime_dir: &Path,
) -> Result<(Option<RuntimeGlExtension>, crate::storage::StorageTimings)> {
    let metadata_path = runtime_dir.join("metadata");
    let metadata = fs::read_to_string(&metadata_path)
        .with_context(|| format!("read runtime metadata {}", metadata_path.display()))?;
    let section = "Extension org.freedesktop.Platform.GL";
    if !metadata_has_section(&metadata, section) {
        return Ok((None, Default::default()));
    }

    let parts = split_runtime_ref(runtime_ref)?;
    let extension_branch = metadata_value(&metadata, section, "versions")
        .and_then(|versions| first_extension_version(&versions))
        .unwrap_or_else(|| parts.branch.clone());
    let directory = metadata_value(&metadata, section, "directory")
        .unwrap_or_else(|| "lib/x86_64-linux-gnu/GL".to_string());
    let runtime_mount_relative = PathBuf::from(directory).join("default");
    let runtime_mountpoint = runtime_dir.join("files").join(&runtime_mount_relative);
    fs::create_dir_all(&runtime_mountpoint).with_context(|| {
        format!(
            "create GL extension mountpoint {}",
            runtime_mountpoint.display()
        )
    })?;

    let ref_name = format!(
        "runtime/org.freedesktop.Platform.GL.default/{}/{}",
        parts.arch, extension_branch
    );
    let checkout_dir = paths.extensions().join(format!(
        "org.freedesktop.Platform.GL.default-{}",
        safe_dir_fragment(&extension_branch)
    ));
    let timings = checkout_if_missing(paths, "extension", &ref_name, None, &checkout_dir, false)?;

    Ok((
        Some(RuntimeGlExtension {
            ref_name,
            checkout_dir,
            runtime_mount_relative,
        }),
        timings,
    ))
}

pub fn ensure_intel_vaapi_extension(
    paths: &Installation,
    runtime_ref: &str,
    runtime_dir: &Path,
) -> Result<Option<RuntimeVaapiExtension>> {
    let metadata_path = runtime_dir.join("metadata");
    let metadata = fs::read_to_string(&metadata_path)
        .with_context(|| format!("read runtime metadata {}", metadata_path.display()))?;
    let section = "Extension org.freedesktop.Platform.VAAPI.Intel";
    if !metadata_has_section(&metadata, section) {
        return Ok(None);
    }

    let parts = split_runtime_ref(runtime_ref)?;
    let extension_branch = metadata_value(&metadata, section, "version")
        .or_else(|| {
            metadata_value(&metadata, section, "versions")
                .and_then(|versions| first_extension_version(&versions))
        })
        .unwrap_or_else(|| parts.branch.clone());
    let directory = metadata_value(&metadata, section, "directory")
        .unwrap_or_else(|| "lib/x86_64-linux-gnu/dri/intel-vaapi-driver".to_string());
    let runtime_mount_relative = PathBuf::from(directory);
    let runtime_mountpoint = runtime_dir.join("files").join(&runtime_mount_relative);
    fs::create_dir_all(&runtime_mountpoint).with_context(|| {
        format!(
            "create VAAPI extension mountpoint {}",
            runtime_mountpoint.display()
        )
    })?;

    let ref_name = format!(
        "runtime/org.freedesktop.Platform.VAAPI.Intel/{}/{}",
        parts.arch, extension_branch
    );
    let checkout_dir = paths.extensions().join(format!(
        "org.freedesktop.Platform.VAAPI.Intel-{}",
        safe_dir_fragment(&extension_branch)
    ));
    checkout_if_missing(paths, "extension", &ref_name, None, &checkout_dir, false)?;

    let ld_library_relative = metadata_value(&metadata, section, "add-ld-path")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);

    Ok(Some(RuntimeVaapiExtension {
        ref_name,
        checkout_dir,
        runtime_mount_relative,
        ld_library_relative,
    }))
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

    for section in metadata_sections_with_prefix(&metadata, "Extension ") {
        let name = section.trim_start_matches("Extension ");
        if name != "org.freedesktop.Platform.ffmpeg-full" {
            continue;
        }

        let Some(directory) = metadata_value(&metadata, &section, "directory") else {
            continue;
        };
        let extension_branch = metadata_value(&metadata, &section, "version")
            .or_else(|| {
                metadata_value(&metadata, &section, "versions")
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
        let ld_library_relative = metadata_value(&metadata, &section, "add-ld-path")
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

pub fn resolve_app(
    paths: &Installation,
    app_id: &str,
    options: ResolveAppOptions,
) -> Result<FlatpakApp> {
    if app_id.contains('/') {
        bail!("app id must not contain '/': {app_id}");
    }

    let app_dir = options.app_dir.unwrap_or_else(|| paths.app(app_id));
    let metadata_path = app_dir.join("metadata");
    let metadata = fs::read_to_string(&metadata_path)
        .with_context(|| format!("read Flatpak metadata {}", metadata_path.display()))?;

    let metadata_app_id = metadata_value(&metadata, "Application", "name").with_context(|| {
        format!(
            "metadata has no Application/name in {}",
            metadata_path.display()
        )
    })?;
    if metadata_app_id != app_id {
        bail!("metadata app id mismatch: requested {app_id}, checkout contains {metadata_app_id}");
    }

    let runtime_ref = metadata_value(&metadata, "Application", "runtime").with_context(|| {
        format!(
            "metadata has no Application/runtime in {}",
            metadata_path.display()
        )
    })?;
    let command = options
        .entry
        .or_else(|| metadata_value(&metadata, "Application", "command"))
        .with_context(|| {
            format!(
                "metadata has no Application/command in {}",
                metadata_path.display()
            )
        })?;
    if command.split_whitespace().count() != 1 {
        bail!("entry command must be a single executable for this POC: {command:?}");
    }

    let runtime_dir = options
        .runtime_dir
        .unwrap_or_else(|| paths.runtimes().join(runtime_checkout_dir(&runtime_ref)));

    validate_checkout_dir("app", &app_dir)?;
    validate_checkout_dir("runtime", &runtime_dir)?;

    Ok(FlatpakApp {
        app_id: app_id.to_string(),
        app_dir,
        runtime_ref,
        runtime_dir,
        command,
        args: options.args,
    })
}

fn checkout_if_missing(
    paths: &Installation,
    kind: &str,
    ref_name: &str,
    expected_checksum: Option<&str>,
    dest: &Path,
    force: bool,
) -> Result<crate::storage::StorageTimings> {
    let (_, summary_path, _) = load_arch_summary(paths)?;
    let summary =
        fs::read(&summary_path).with_context(|| format!("read {}", summary_path.display()))?;
    let refs = parse_summary_refs(&summary_path)?;
    let checksum = match expected_checksum {
        Some(checksum) => checksum,
        None => refs
            .iter()
            .find(|candidate| candidate.name == ref_name)
            .map(|candidate| candidate.checksum.as_str())
            .with_context(|| format!("ref is not present in Flathub: {ref_name}"))?,
    };
    Storage::open(paths)?.deploy(
        &summary,
        &[Deployment {
            kind,
            ref_name,
            checksum,
            destination: dest,
            force,
        }],
    )
}

fn fetch_summary_index(paths: &Installation) -> Result<PathBuf> {
    let remote_dir = paths.remote_metadata();
    let path = remote_dir.join("summary.idx");
    let signature_path = remote_dir.join("summary.idx.sig");
    let checked = remote_dir.join("summary.idx.checked");
    fs::create_dir_all(&remote_dir).context("create remote metadata directory")?;
    if signature_path.is_file() && metadata_is_fresh(&path, &checked)? {
        return Ok(path);
    }
    let _lock = MetadataLock::acquire(&remote_dir.join("summary.idx.lock"))?;
    if signature_path.is_file() && metadata_is_fresh(&path, &checked)? {
        return Ok(path);
    }
    let candidate = remote_dir.join("summary.idx.candidate");
    let signature_candidate = remote_dir.join("summary.idx.sig.candidate");
    let refresh = || -> Result<()> {
        fetch_metadata_file(
            &format!("{REMOTE}/summary.idx"),
            &candidate,
            "Flathub metadata index",
        )?;
        fetch_metadata_file(
            &format!("{REMOTE}/summary.idx.sig"),
            &signature_candidate,
            "Flathub metadata signature",
        )?;
        let summary =
            fs::read(&candidate).with_context(|| format!("read {}", candidate.display()))?;
        let signatures = fs::read(&signature_candidate)
            .with_context(|| format!("read {}", signature_candidate.display()))?;
        Storage::open(paths)?.verify_summary(&summary, &signatures)?;
        fs::rename(&candidate, &path).with_context(|| format!("activate {}", path.display()))?;
        fs::rename(&signature_candidate, &signature_path)
            .with_context(|| format!("activate {}", signature_path.display()))?;
        Ok(())
    };
    match refresh() {
        Ok(()) => {
            fs::write(&checked, format!("{}\n", unix_timestamp()))
                .with_context(|| format!("write {}", checked.display()))?;
        }
        Err(error) if path.is_file() => {
            eprintln!("warning: refresh Flathub metadata index failed; using cache: {error:#}");
        }
        Err(error) => return Err(error),
    }
    Ok(path)
}

fn fetch_subsummary(paths: &Installation, arch: &str, digest: &str) -> Result<PathBuf> {
    let summaries = paths.remote_metadata().join("summaries");
    fs::create_dir_all(&summaries).context("create architecture summary cache")?;
    let path = summaries.join(format!("{digest}.gz"));
    if path.is_file() {
        return Ok(path);
    }
    let _lock = MetadataLock::acquire(&summaries.join(format!("{digest}.lock")))?;
    if path.is_file() {
        return Ok(path);
    }
    fetch_metadata_file(
        &format!("{REMOTE}/summaries/{digest}.gz"),
        &path,
        &format!("Flathub metadata for {arch}"),
    )?;
    Ok(path)
}

fn summary_digest_matches(data: &[u8], expected: &str) -> Result<bool> {
    let mut checksum = Checksum::new(ChecksumType::Sha256).context("create SHA-256 checksum")?;
    checksum.update(data);
    Ok(checksum
        .string()
        .is_some_and(|actual| actual.eq_ignore_ascii_case(expected)))
}

fn cache_uncompressed_subsummary(
    paths: &Installation,
    digest: &str,
    compressed_path: &Path,
) -> Result<PathBuf> {
    let path = paths
        .remote_metadata()
        .join("summaries")
        .join(format!("{digest}.sub"));
    if path.is_file() {
        let data = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        if summary_digest_matches(&data, digest)? {
            return Ok(path);
        }
        fs::remove_file(&path).with_context(|| format!("remove invalid {}", path.display()))?;
    }
    let _lock = MetadataLock::acquire(&path.with_extension("sub.lock"))?;
    if path.is_file() {
        let data = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        if summary_digest_matches(&data, digest)? {
            return Ok(path);
        }
        fs::remove_file(&path).with_context(|| format!("remove invalid {}", path.display()))?;
    }
    let compressed =
        fs::read(compressed_path).with_context(|| format!("read {}", compressed_path.display()))?;
    let data = decompress_gzip(&compressed)
        .with_context(|| format!("decompress {}", compressed_path.display()))?;
    if !summary_digest_matches(&data, digest)? {
        let _ = fs::remove_file(compressed_path);
        bail!("downloaded Flathub architecture summary failed SHA-256 verification");
    }
    let partial = path.with_extension("sub.part");
    fs::write(&partial, data).with_context(|| format!("write {}", partial.display()))?;
    fs::rename(&partial, &path)
        .with_context(|| format!("complete summary cache {}", path.display()))?;
    Ok(path)
}

fn fetch_metadata_file(url: &str, path: &Path, label: &str) -> Result<()> {
    let partial = path.with_file_name(format!(
        ".{}.part",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("metadata")
    ));
    println!("  Downloading {label}...");
    let _ = std::io::stdout().flush();

    let mut command = Command::new("fetch");
    command
        .arg("-a")
        .arg("-F")
        .arg("-r")
        .arg("-R")
        .arg("-q")
        .arg("-o")
        .arg(&partial);
    let mut child = command
        .arg(url)
        .spawn()
        .with_context(|| format!("download {label}"))?;
    let terminal = std::io::stdout().is_terminal();
    let mut last_bytes = None;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .with_context(|| format!("wait for {label} download"))?
        {
            break status;
        }
        if terminal {
            let bytes = fs::metadata(&partial)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            if last_bytes != Some(bytes) {
                print!("\r\x1b[2K    Received {}", format_byte_count(bytes));
                let _ = std::io::stdout().flush();
                last_bytes = Some(bytes);
            }
        }
        std::thread::sleep(Duration::from_millis(150));
    };
    let bytes = fs::metadata(&partial)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if terminal {
        print!("\r\x1b[2K");
    }
    if !status.success() {
        bail!("download {label} failed with status {status}; partial download kept for retry");
    }
    fs::rename(&partial, path)
        .with_context(|| format!("complete metadata download {}", path.display()))?;
    println!("  Downloaded {label} ({})", format_byte_count(bytes));
    let _ = std::io::stdout().flush();
    Ok(())
}

fn format_byte_count(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}

struct MetadataLock {
    path: PathBuf,
}

impl MetadataLock {
    fn acquire(path: &Path) -> Result<Self> {
        let started = std::time::Instant::now();
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id())?;
                    return Ok(Self {
                        path: path.to_path_buf(),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if stale_metadata_lock(path) {
                        let _ = fs::remove_file(path);
                        continue;
                    }
                    if started.elapsed() > std::time::Duration::from_secs(300) {
                        bail!(
                            "timed out waiting for metadata refresh lock {}",
                            path.display()
                        );
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(error) => {
                    return Err(error).with_context(|| format!("create {}", path.display()))
                }
            }
        }
    }
}

impl Drop for MetadataLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn stale_metadata_lock(path: &Path) -> bool {
    let Ok(pid) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(pid) = pid.trim().parse::<i32>() else {
        return false;
    };
    unsafe {
        libc::kill(pid, 0) != 0
            && std::io::Error::last_os_error().raw_os_error() != Some(libc::EPERM)
    }
}

fn metadata_is_fresh(path: &Path, checked: &Path) -> Result<bool> {
    if !path.is_file() || !checked.is_file() {
        return Ok(false);
    }
    let ttl = std::env::var("FREEBSD_FLATPAK_METADATA_TTL")
        .ok()
        .map(|value| {
            value
                .parse::<u64>()
                .context("parse FREEBSD_FLATPAK_METADATA_TTL")
        })
        .transpose()?
        .unwrap_or(300);
    let Ok(timestamp) = fs::read_to_string(checked)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .ok_or(())
    else {
        return Ok(false);
    };
    Ok(unix_timestamp().saturating_sub(timestamp) < ttl)
}

fn unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn parse_summary_refs(path: &Path) -> Result<Vec<RemoteRef>> {
    let data = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    parse_summary_refs_bytes(data)
        .with_context(|| format!("parse Flathub architecture summary {}", path.display()))
}

fn parse_summary_refs_bytes(data: Vec<u8>) -> Result<Vec<RemoteRef>> {
    let variant = variant_from_bytes(data, "(a(s(taya{sv}))a{sv})")?;
    let refs_v = variant.child_value(0);
    let mut refs = Vec::with_capacity(refs_v.n_children());

    for i in 0..refs_v.n_children() {
        let item = refs_v.child_value(i);
        let name = item.child_value(0).str().unwrap_or_default().to_string();
        let info = item.child_value(1);
        refs.push(remote_ref_from_summary_info(name, &info)?);
    }

    Ok(refs)
}

fn remote_ref_from_summary_info(name: String, info: &Variant) -> Result<RemoteRef> {
    let map = info.child_value(2);
    let (installed_size, download_size) = lookup_variant_value(&map, "xa.data")
        .filter(|data| data.n_children() == 3)
        .map(|data| {
            (
                data.child_value(0).get::<u64>().map(u64::from_be),
                data.child_value(1).get::<u64>().map(u64::from_be),
            )
        })
        .unwrap_or((None, None));
    Ok(RemoteRef {
        name,
        checksum: bytes_to_checksum(&info.child_value(1))?,
        metadata: lookup_flatpak_metadata(&map),
        download_size,
        installed_size,
    })
}

fn parse_summary_index(path: &Path, arch: &str) -> Result<(String, Option<String>)> {
    let variant = variant_from_file(path, "(a{s(ayaaya{sv})}a{sv})")
        .with_context(|| format!("parse Flathub summary index {}", path.display()))?;
    let collection_id =
        lookup_variant_string(&variant.child_value(1), "ostree.summary.collection-id");
    let summaries = variant.child_value(0);
    for index in 0..summaries.n_children() {
        let entry = summaries.child_value(index);
        if entry.child_value(0).str() != Some(arch) {
            continue;
        }
        let details = entry.child_value(1);
        return Ok((bytes_to_checksum(&details.child_value(0))?, collection_id));
    }
    bail!("Flathub summary index has no metadata for architecture {arch}")
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

fn fetch_appstream_replacements(
    remote_dir: &Path,
    arch: &str,
) -> Result<BTreeMap<String, Vec<String>>> {
    let path = fetch_appstream(remote_dir, arch)?;
    let compressed = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let xml = decompress_appstream_xml(&compressed)
        .with_context(|| format!("decompress {}", path.display()))?;
    Ok(parse_appstream_replacements(&xml))
}

fn fetch_appstream(remote_dir: &Path, arch: &str) -> Result<PathBuf> {
    let path = remote_dir.join(format!("appstream-{}.xml.gz", safe_dir_fragment(arch)));
    let checked = remote_dir.join(format!("appstream-{}.checked", safe_dir_fragment(arch)));
    fs::create_dir_all(remote_dir).context("create remote metadata directory")?;
    if metadata_is_fresh(&path, &checked)? {
        return Ok(path);
    }
    let _lock = MetadataLock::acquire(
        &remote_dir.join(format!("appstream-{}.lock", safe_dir_fragment(arch))),
    )?;
    if metadata_is_fresh(&path, &checked)? {
        return Ok(path);
    }
    match fetch_metadata_file(
        &format!("{REMOTE}/appstream/{arch}/appstream.xml.gz"),
        &path,
        "Flathub AppStream metadata",
    ) {
        Ok(()) => {
            fs::write(&checked, format!("{}\n", unix_timestamp()))
                .with_context(|| format!("write {}", checked.display()))?;
        }
        Err(error) if path.is_file() => {
            eprintln!(
                "warning: refresh Flathub app replacement metadata failed; using cache: {error:#}"
            );
        }
        Err(error) => return Err(error),
    }
    Ok(path)
}

fn decompress_appstream_xml(data: &[u8]) -> Result<String> {
    String::from_utf8(decompress_gzip(data)?).context("AppStream XML is not UTF-8")
}

fn decompress_gzip(data: &[u8]) -> Result<Vec<u8>> {
    let payload = gzip_deflate_payload(data)?;
    decompress_to_vec(payload).map_err(|error| anyhow::anyhow!("inflate gzip payload: {error:?}"))
}

fn gzip_deflate_payload(data: &[u8]) -> Result<&[u8]> {
    if data.len() < 18 || data[0] != 0x1f || data[1] != 0x8b {
        bail!("not a gzip stream");
    }
    if data[2] != 8 {
        bail!("unsupported gzip compression method {}", data[2]);
    }

    let flags = data[3];
    let mut offset = 10usize;
    if flags & 0x04 != 0 {
        if data.len() < offset + 2 {
            bail!("truncated gzip extra header");
        }
        let extra_len = u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2 + extra_len;
    }
    if flags & 0x08 != 0 {
        offset = skip_gzip_c_string(data, offset).context("truncated gzip file name")?;
    }
    if flags & 0x10 != 0 {
        offset = skip_gzip_c_string(data, offset).context("truncated gzip comment")?;
    }
    if flags & 0x02 != 0 {
        offset += 2;
    }
    if data.len() < offset + 8 {
        bail!("truncated gzip payload");
    }

    Ok(&data[offset..data.len() - 8])
}

fn skip_gzip_c_string(data: &[u8], offset: usize) -> Option<usize> {
    data.get(offset..)?
        .iter()
        .position(|byte| *byte == 0)
        .map(|position| offset + position + 1)
}

fn parse_appstream_replacements(xml: &str) -> BTreeMap<String, Vec<String>> {
    let mut replacements: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut rest = xml;

    while let Some(start) = rest.find("<component") {
        rest = &rest[start..];
        let Some(tag_end) = rest.find('>') else {
            break;
        };
        let component_body = &rest[tag_end + 1..];
        let Some(end) = component_body.find("</component>") else {
            break;
        };
        let component = &component_body[..end];
        rest = &component_body[end + "</component>".len()..];

        let Some(new_id) = first_xml_text(component, "id") else {
            continue;
        };
        let mut component_rest = component;
        while let Some(replaces_start) = component_rest.find("<replaces>") {
            component_rest = &component_rest[replaces_start + "<replaces>".len()..];
            let Some(replaces_end) = component_rest.find("</replaces>") else {
                break;
            };
            let replaces = &component_rest[..replaces_end];
            component_rest = &component_rest[replaces_end + "</replaces>".len()..];

            for old_id in xml_texts(replaces, "id") {
                replacements.entry(old_id).or_default().push(new_id.clone());
            }
        }
    }

    replacements
}

fn parse_appstream_info(xml: &str, app_id: &str) -> Option<AppstreamInfo> {
    let mut rest = xml;
    while let Some(start) = rest.find("<component") {
        rest = &rest[start..];
        let tag_end = rest.find('>')?;
        let component_body = &rest[tag_end + 1..];
        let end = component_body.find("</component>")?;
        let component = &component_body[..end];
        rest = &component_body[end + "</component>".len()..];
        if first_xml_text(component, "id").as_deref() != Some(app_id) {
            continue;
        }

        let version = first_release_version(component);
        return Some(AppstreamInfo {
            name: first_xml_text(component, "name"),
            summary: first_xml_text(component, "summary"),
            version,
            license: first_xml_text(component, "project_license"),
        });
    }
    None
}

fn first_release_version(component: &str) -> Option<String> {
    let mut rest = component;
    while let Some(start) = rest.find("<release") {
        rest = &rest[start..];
        let end = rest.find('>')?;
        let tag = &rest[..=end];
        if let Some(version) = xml_attribute(tag, "version") {
            return Some(version);
        }
        rest = &rest[end + 1..];
    }
    None
}

fn xml_attribute(tag: &str, attribute: &str) -> Option<String> {
    let needle = format!("{attribute}=\"");
    let value = &tag[tag.find(&needle)? + needle.len()..];
    let end = value.find('"')?;
    let value = value[..end].trim();
    (!value.is_empty()).then(|| xml_unescape_text(value))
}

fn first_xml_text(xml: &str, tag: &str) -> Option<String> {
    xml_texts(xml, tag).into_iter().next()
}

fn xml_texts(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut values = Vec::new();
    let mut rest = xml;

    while let Some(start) = rest.find(&open) {
        let value_start = start + open.len();
        let after_open = &rest[value_start..];
        let Some(end) = after_open.find(&close) else {
            break;
        };
        let value = after_open[..end].trim();
        if !value.is_empty() {
            values.push(xml_unescape_text(value));
        }
        rest = &after_open[end + close.len()..];
    }

    values
}

fn xml_unescape_text(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
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

fn host_flatpak_arch() -> Result<String> {
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

pub fn metadata_value(metadata: &str, section: &str, key: &str) -> Option<String> {
    let mut current = "";
    for line in metadata.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            current = &line[1..line.len() - 1];
            continue;
        }
        if current == section {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            if k.trim() == key {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

pub fn metadata_section_entries(metadata: &str, section: &str) -> Vec<(String, String)> {
    let mut current = "";
    let mut entries = Vec::new();
    for line in metadata.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            current = &line[1..line.len() - 1];
            continue;
        }
        if current != section {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        entries.push((key.to_string(), value.trim().to_string()));
    }
    entries
}

pub fn metadata_has_section(metadata: &str, section: &str) -> bool {
    metadata.lines().any(|line| {
        let line = line.trim();
        line.starts_with('[') && line.ends_with(']') && &line[1..line.len() - 1] == section
    })
}

fn metadata_sections_with_prefix(metadata: &str, prefix: &str) -> Vec<String> {
    metadata
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('[') && line.ends_with(']') {
                let section = &line[1..line.len() - 1];
                if section.starts_with(prefix) {
                    return Some(section.to_string());
                }
            }
            None
        })
        .collect()
}

pub fn runtime_checkout_dir(runtime_ref: &str) -> String {
    let mut parts = runtime_ref.split('/');
    let name = parts.next().unwrap_or(runtime_ref);
    let _arch = parts.next();
    let branch = parts.next().unwrap_or("stable");
    format!("{name}-{}", branch.replace('/', "_"))
}

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
        for section in metadata_sections_with_prefix(&metadata, "Extension ") {
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
        let versions = metadata_value(metadata, section, "version")
            .into_iter()
            .chain(
                metadata_value(metadata, section, "versions")
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
            metadata_value(metadata, section, key).is_some_and(|value| {
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
            subdirectories: metadata_value(metadata, section, "subdirectories")
                .is_some_and(|value| value == "true"),
            active_gl_driver_condition: condition_is_active_gl_driver("download-if")
                || condition_is_active_gl_driver("enable-if"),
            autoprune_unless_active_gl_driver: metadata_value(
                metadata,
                section,
                "autoprune-unless",
            )
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

fn parse_runtime_ref(ref_name: &str) -> Option<RuntimeRefParts> {
    let runtime_ref = ref_name.strip_prefix("runtime/")?;
    split_runtime_ref(runtime_ref).ok()
}

struct RuntimeRefParts {
    name: String,
    arch: String,
    branch: String,
}

fn split_runtime_ref(runtime_ref: &str) -> Result<RuntimeRefParts> {
    let mut parts = runtime_ref.splitn(3, '/');
    let name = parts.next().context("missing runtime name")?;
    let arch = parts.next().context("missing runtime arch")?;
    let branch = parts.next().context("missing runtime branch")?;
    Ok(RuntimeRefParts {
        name: name.to_string(),
        arch: arch.to_string(),
        branch: branch.to_string(),
    })
}

fn first_extension_version(versions: &str) -> Option<String> {
    versions
        .split(';')
        .map(str::trim)
        .find(|version| !version.is_empty())
        .map(ToOwned::to_owned)
}

fn safe_dir_fragment(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn validate_checkout_dir(kind: &str, dir: &Path) -> Result<()> {
    if !dir.is_dir() {
        bail!(
            "{kind} checkout directory does not exist: {}",
            dir.display()
        );
    }
    let files = dir.join("files");
    if !files.is_dir() {
        bail!(
            "{kind} checkout is missing files directory: {}",
            files.display()
        );
    }
    Ok(())
}

fn variant_from_file(path: &Path, ty: &'static str) -> Result<Variant> {
    let data = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    variant_from_bytes(data, ty)
}

fn variant_from_bytes(data: Vec<u8>, ty: &'static str) -> Result<Variant> {
    let bytes = Bytes::from_owned(data);
    let ty = VariantTy::new(ty).context("invalid GVariant type")?;
    Ok(Variant::from_bytes_with_type(&bytes, ty))
}

fn lookup_variant_string(map: &Variant, key: &str) -> Option<String> {
    lookup_variant_value(map, key)?
        .str()
        .map(ToString::to_string)
}

fn lookup_flatpak_metadata(map: &Variant) -> Option<String> {
    if let Some(metadata) = lookup_variant_string(map, "xa.metadata") {
        return Some(metadata);
    }
    let data = lookup_variant_value(map, "xa.data")?;
    if data.n_children() != 3 {
        return None;
    }
    data.child_value(2).str().map(ToString::to_string)
}

fn lookup_variant_value(map: &Variant, key: &str) -> Option<Variant> {
    for i in 0..map.n_children() {
        let entry = map.child_value(i);
        let key_variant = entry.child_value(0);
        let entry_key = key_variant.str()?;
        if entry_key != key {
            continue;
        }
        let boxed = entry.child_value(1);
        return boxed.as_variant();
    }
    None
}

fn bytes_to_checksum(variant: &Variant) -> Result<String> {
    let bytes = variant.data_as_bytes();
    let data = bytes.as_ref();
    if data.len() != 32 {
        bail!("expected 32-byte checksum, got {}", data.len());
    }
    Ok(data.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::{
        generation_checkout_dir, parse_appstream_info, parse_appstream_replacements,
        required_extension_refs, resolve_current_app_id_from_replacements, select_history_commit,
        RemoteMetadata, RemoteRef,
    };
    use crate::storage::CommitInfo;
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;

    #[test]
    fn active_gl_default_subextension_is_required_by_runtime_metadata() {
        let root = std::env::temp_dir().join(format!(
            "freebsd-flatpak-runtime-gl-reachability-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let app = root.join("app");
        let runtime = root.join("runtime");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&runtime).unwrap();
        fs::write(
            app.join("metadata"),
            "[Application]\nname=org.example.App\n",
        )
        .unwrap();
        fs::write(
            runtime.join("metadata"),
            "[Runtime]\nname=org.freedesktop.Platform\n\n[Extension org.freedesktop.Platform.GL]\ndirectory=lib/x86_64-linux-gnu/GL\nversions=25.08;25.08-extra;1.4\nsubdirectories=true\ndownload-if=active-gl-driver\nenable-if=active-gl-driver\nautoprune-unless=active-gl-driver\n",
        )
        .unwrap();
        let gl_default = "runtime/org.freedesktop.Platform.GL.default/x86_64/25.08".to_string();
        let installed = BTreeSet::from([
            gl_default.clone(),
            "runtime/org.freedesktop.Platform.GL.default/x86_64/24.08".to_string(),
            "runtime/org.freedesktop.Platform.GL.vendor/x86_64/25.08".to_string(),
        ]);

        let required = required_extension_refs(
            &app,
            "org.freedesktop.Platform/x86_64/25.08",
            &runtime,
            &installed,
        )
        .unwrap();
        assert_eq!(required, BTreeSet::from([gl_default]));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn historical_commit_uses_an_immutable_generation_path() {
        let root = std::env::temp_dir().join(format!(
            "freebsd-flatpak-runtime-generation-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let base = root.join("org.example.App");
        fs::create_dir_all(base.join("commit-b")).unwrap();

        assert_eq!(
            generation_checkout_dir(&base, "commit-a", false),
            base.join("commit-a")
        );
        assert_eq!(
            generation_checkout_dir(&base, "commit-b", false),
            base.join("commit-b")
        );
        let repaired = generation_checkout_dir(&base, "commit-b", true);
        assert_ne!(repaired, base.join("commit-b"));
        assert!(repaired.starts_with(&base));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn exact_app_ref_does_not_require_appstream_metadata() {
        let metadata = RemoteMetadata {
            arch: "x86_64".to_string(),
            refs: vec![RemoteRef {
                name: "app/org.example.App/x86_64/stable".to_string(),
                checksum: "app-commit".to_string(),
                metadata: None,
                download_size: None,
                installed_size: None,
            }],
            remote_dir: std::path::PathBuf::from("/dev/null"),
            summary_path: std::path::PathBuf::from("/dev/null"),
            collection_id: None,
        };

        let remote_ref = metadata.resolve_app_ref("org.example.App", true).unwrap();
        assert_eq!(remote_ref.name, "app/org.example.App/x86_64/stable");
    }

    fn commit(checksum: &str) -> CommitInfo {
        CommitInfo {
            checksum: checksum.to_string(),
            parent: None,
            timestamp: 0,
            subject: String::new(),
            body: String::new(),
            flatpak_metadata: None,
            version: None,
            collection_id: None,
        }
    }

    #[test]
    fn historical_commit_selection_accepts_unique_prefixes() {
        let history = vec![
            commit("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            commit("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        ];

        assert_eq!(
            select_history_commit(&history, "BBBBBBBBBBBB")
                .unwrap()
                .checksum,
            history[1].checksum
        );
    }

    #[test]
    fn historical_commit_selection_rejects_commits_outside_ref_history() {
        let history = vec![commit(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )];

        let error = select_history_commit(&history, "bbbbbbbbbbbb").unwrap_err();
        assert!(error.to_string().contains("not in the history"));
    }

    #[test]
    fn appstream_replacements_map_old_ids_to_current_component() {
        let xml = r#"
<components>
  <component type="desktop-application">
    <id>app.example.Current</id>
    <name>Example</name>
    <replaces>
      <id>org.example.Old</id>
      <id>org.example.Older</id>
    </replaces>
  </component>
</components>
"#;

        let replacements = parse_appstream_replacements(xml);

        assert_eq!(
            replacements.get("org.example.Old").unwrap(),
            &vec!["app.example.Current".to_string()]
        );
        assert_eq!(
            replacements.get("org.example.Older").unwrap(),
            &vec!["app.example.Current".to_string()]
        );
    }

    #[test]
    fn appstream_info_reads_display_fields_and_latest_version() {
        let xml = r#"
<components>
  <component type="desktop-application">
    <id>org.example.App</id>
    <name>Example &amp; More</name>
    <summary>Do useful things</summary>
    <project_license>GPL-3.0-or-later</project_license>
    <releases>
      <release version="50.0" date="2026-01-01"/>
      <release version="49.0" date="2025-01-01"/>
    </releases>
  </component>
</components>
"#;

        let info = parse_appstream_info(xml, "org.example.App").unwrap();

        assert_eq!(info.name.as_deref(), Some("Example & More"));
        assert_eq!(info.summary.as_deref(), Some("Do useful things"));
        assert_eq!(info.version.as_deref(), Some("50.0"));
        assert_eq!(info.license.as_deref(), Some("GPL-3.0-or-later"));
    }

    #[test]
    fn appstream_info_preserves_missing_optional_fields() {
        let xml = r#"
<components>
  <component type="desktop-application">
    <id>org.example.App</id>
    <name>Example</name>
  </component>
</components>
"#;

        let info = parse_appstream_info(xml, "org.example.App").unwrap();

        assert_eq!(info.name.as_deref(), Some("Example"));
        assert_eq!(info.summary, None);
        assert_eq!(info.version, None);
        assert_eq!(info.license, None);
        assert!(parse_appstream_info(xml, "org.example.Missing").is_none());
    }

    #[test]
    fn current_app_id_follows_available_replacement() {
        let refs = vec![RemoteRef {
            name: "app/app.example.Current/x86_64/stable".to_string(),
            checksum: "app-2".to_string(),
            metadata: None,
            download_size: None,
            installed_size: None,
        }];
        let replacements = BTreeMap::from([(
            "org.example.Old".to_string(),
            vec!["app.example.Current".to_string()],
        )]);

        assert_eq!(
            resolve_current_app_id_from_replacements(
                &refs,
                &replacements,
                "org.example.Old",
                "x86_64"
            )
            .unwrap(),
            "app.example.Current"
        );
    }

    #[test]
    fn current_app_id_ignores_unavailable_replacement() {
        let refs = vec![RemoteRef {
            name: "app/app.example.Current/aarch64/stable".to_string(),
            checksum: "app-2".to_string(),
            metadata: None,
            download_size: None,
            installed_size: None,
        }];
        let replacements = BTreeMap::from([(
            "org.example.Old".to_string(),
            vec!["app.example.Current".to_string()],
        )]);

        assert_eq!(
            resolve_current_app_id_from_replacements(
                &refs,
                &replacements,
                "org.example.Old",
                "x86_64"
            )
            .unwrap(),
            "org.example.Old"
        );
    }

    #[test]
    fn current_app_id_rejects_ambiguous_replacements() {
        let refs = vec![
            RemoteRef {
                name: "app/app.example.One/x86_64/stable".to_string(),
                checksum: "app-1".to_string(),
                metadata: None,
                download_size: None,
                installed_size: None,
            },
            RemoteRef {
                name: "app/app.example.Two/x86_64/stable".to_string(),
                checksum: "app-2".to_string(),
                metadata: None,
                download_size: None,
                installed_size: None,
            },
        ];
        let replacements = BTreeMap::from([(
            "org.example.Old".to_string(),
            vec!["app.example.One".to_string(), "app.example.Two".to_string()],
        )]);

        let error = resolve_current_app_id_from_replacements(
            &refs,
            &replacements,
            "org.example.Old",
            "x86_64",
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("multiple Flathub replacements found"));
    }

    #[test]
    fn current_app_id_rejects_replacement_cycles() {
        let refs = vec![
            RemoteRef {
                name: "app/org.example.A/x86_64/stable".to_string(),
                checksum: "app-a".to_string(),
                metadata: None,
                download_size: None,
                installed_size: None,
            },
            RemoteRef {
                name: "app/org.example.B/x86_64/stable".to_string(),
                checksum: "app-b".to_string(),
                metadata: None,
                download_size: None,
                installed_size: None,
            },
        ];
        let replacements = BTreeMap::from([
            (
                "org.example.A".to_string(),
                vec!["org.example.B".to_string()],
            ),
            (
                "org.example.B".to_string(),
                vec!["org.example.A".to_string()],
            ),
        ]);

        let error = resolve_current_app_id_from_replacements(
            &refs,
            &replacements,
            "org.example.A",
            "x86_64",
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("cycle in Flathub replacement metadata"));
    }
}
