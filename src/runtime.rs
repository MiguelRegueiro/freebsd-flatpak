use crate::paths::Installation;
use anyhow::{bail, Context, Result};
use glib::{Bytes, Variant, VariantTy};
use miniz_oxide::inflate::decompress_to_vec;
use miniz_oxide::inflate::decompress_to_vec_zlib;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io::Write;
use std::os::unix::fs as unix_fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

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

#[derive(Debug, Clone)]
pub struct RemoteApp {
    pub app_id: String,
    pub app_ref: String,
    pub app_commit: String,
    pub arch: String,
    pub branch: String,
    pub runtime_ref: String,
    pub runtime_commit: String,
    pub command: String,
}

#[derive(Debug, Clone)]
struct RemoteRef {
    name: String,
    checksum: String,
}

#[derive(Debug, Clone)]
pub struct RemoteMetadata {
    arch: String,
    refs: Vec<RemoteRef>,
    objects: PathBuf,
    remote_dir: PathBuf,
}

impl RemoteMetadata {
    pub fn resolve_exact_ref(&self, app_ref: &str) -> Result<RemoteApp> {
        let remote_ref = self
            .refs
            .iter()
            .find(|item| item.name == app_ref)
            .cloned()
            .with_context(|| format!("app ref is no longer present in Flathub: {app_ref}"))?;
        remote_app_from_ref(&self.refs, remote_ref, &self.arch, &self.objects)
    }

    pub fn resolve_app(&self, app_id: &str, replacements: bool) -> Result<RemoteApp> {
        if app_id.contains('/') {
            bail!("app id must not contain '/': {app_id}");
        }
        let app_id = if replacements {
            resolve_current_app_id(&self.refs, app_id, &self.arch, &self.remote_dir)?
        } else {
            app_id.to_string()
        };
        let app_ref = choose_app_ref(&self.refs, &app_id, &self.arch)?;
        remote_app_from_ref(&self.refs, app_ref, &self.arch, &self.objects)
    }
}

#[derive(Debug, Clone)]
struct Commit {
    checksum: String,
    tree: String,
    dirmeta: String,
    metadata: String,
}

#[derive(Debug, Clone)]
struct FileEntry {
    name: String,
    checksum: String,
}

#[derive(Debug, Clone)]
struct DirEntry {
    name: String,
    tree: String,
    _dirmeta: String,
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

    for ref_name in refs {
        let checksum = fetch_ref(&ref_name)?;
        let commit = fetch_commit(&paths.objects(), &checksum)?;
        println!("{ref_name}");
        println!("  commit: {}", commit.checksum);
        println!("  tree: {}", commit.tree);
        println!("  dirmeta: {}", commit.dirmeta);
        if let Some(command) = metadata_value(&commit.metadata, "Application", "command") {
            println!("  command: {command}");
        }
        if let Some(runtime) = metadata_value(&commit.metadata, "Application", "runtime") {
            println!("  runtime: {runtime}");
        }
    }
    Ok(())
}

pub fn install_app(paths: &Installation, app_id: &str) -> Result<InstalledApp> {
    let remote = resolve_remote_app(paths, app_id)?;
    checkout_remote_app(paths, &remote, false, false)
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
    load_remote_metadata(paths)?.resolve_app(app_id, true)
}

pub fn load_remote_metadata(paths: &Installation) -> Result<RemoteMetadata> {
    let arch = host_flatpak_arch()?;
    let summary_path = fetch_summary(paths)?;
    let refs = parse_summary_refs(&summary_path)?;
    Ok(RemoteMetadata {
        arch,
        refs,
        objects: paths.objects(),
        remote_dir: paths.remote_metadata(),
    })
}

fn remote_app_from_ref(
    refs: &[RemoteRef],
    app_remote_ref: RemoteRef,
    arch: &str,
    objects: &Path,
) -> Result<RemoteApp> {
    let app_ref_parts = split_flatpak_ref(&app_remote_ref.name)?;
    let app_id = app_ref_parts.name;
    let app_commit = fetch_commit(objects, &app_remote_ref.checksum)?;
    let metadata_app_id = metadata_value(&app_commit.metadata, "Application", "name")
        .context("remote app metadata has no Application/name")?;
    if metadata_app_id != app_id {
        bail!("remote metadata app id mismatch: requested {app_id}, found {metadata_app_id}");
    }

    let runtime_ref = metadata_value(&app_commit.metadata, "Application", "runtime")
        .context("remote app metadata has no Application/runtime")?;
    let command = metadata_value(&app_commit.metadata, "Application", "command")
        .context("remote app metadata has no Application/command")?;
    if command.split_whitespace().count() != 1 {
        bail!("entry command must be a single executable for this POC: {command:?}");
    }

    let runtime_full_ref = format!("runtime/{runtime_ref}");
    let runtime_remote_ref = refs
        .iter()
        .find(|remote_ref| remote_ref.name == runtime_full_ref)
        .cloned()
        .with_context(|| {
            format!("required runtime ref not found in Flathub summary: {runtime_full_ref}")
        })?;
    Ok(RemoteApp {
        app_id,
        app_ref: app_remote_ref.name,
        app_commit: app_remote_ref.checksum,
        arch: arch.to_string(),
        branch: app_ref_parts.branch,
        runtime_ref,
        runtime_commit: runtime_remote_ref.checksum,
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
    let app_dir = paths.app(&remote.app_id);
    let runtime_dir = paths
        .runtimes()
        .join(runtime_checkout_dir(&remote.runtime_ref));

    checkout_if_missing(paths, &remote.app_ref, &app_dir, force_app)?;
    checkout_if_missing(
        paths,
        &format!("runtime/{}", remote.runtime_ref),
        &runtime_dir,
        force_runtime,
    )?;
    let _ = ensure_default_gl_extension(paths, &remote.runtime_ref, &runtime_dir)?;

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
    })
}

pub fn ensure_default_gl_extension(
    paths: &Installation,
    runtime_ref: &str,
    runtime_dir: &Path,
) -> Result<Option<RuntimeGlExtension>> {
    let metadata_path = runtime_dir.join("metadata");
    let metadata = fs::read_to_string(&metadata_path)
        .with_context(|| format!("read runtime metadata {}", metadata_path.display()))?;
    let section = "Extension org.freedesktop.Platform.GL";
    if !metadata_has_section(&metadata, section) {
        return Ok(None);
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
    checkout_if_missing(paths, &ref_name, &checkout_dir, false)?;

    Ok(Some(RuntimeGlExtension {
        ref_name,
        checkout_dir,
        runtime_mount_relative,
    }))
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
    checkout_if_missing(paths, &ref_name, &checkout_dir, false)?;

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
        checkout_if_missing(paths, &ref_name, &checkout_dir, false)?;
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
    ref_name: &str,
    dest: &Path,
    force: bool,
) -> Result<()> {
    if !force && dest.join("metadata").is_file() && dest.join("files").is_dir() {
        println!("reusing checkout for {ref_name}: {}", dest.display());
        return Ok(());
    }
    if force && dest.exists() {
        fs::remove_dir_all(dest)
            .with_context(|| format!("remove old checkout {}", dest.display()))?;
    }
    checkout_ref(paths, ref_name, dest.to_path_buf())
}

fn fetch_summary(paths: &Installation) -> Result<PathBuf> {
    let remote_dir = paths.remote_metadata();
    let path = remote_dir.join("summary");
    let checked = remote_dir.join("summary.checked");
    fs::create_dir_all(&remote_dir).context("create remote metadata directory")?;
    if metadata_is_fresh(&path, &checked)? {
        return Ok(path);
    }
    let _lock = MetadataLock::acquire(&remote_dir.join("summary.lock"))?;
    if metadata_is_fresh(&path, &checked)? {
        return Ok(path);
    }
    let tmp = path.with_file_name(format!(
        ".summary.{}.{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));

    let status = Command::new("fetch")
        .arg("-qo")
        .arg(&tmp)
        .arg(format!("{REMOTE}/summary"))
        .status()
        .context("fetch Flathub summary")?;
    if status.success() {
        fs::rename(&tmp, &path)
            .with_context(|| format!("move {} to {}", tmp.display(), path.display()))?;
        fs::write(&checked, format!("{}\n", unix_timestamp()))
            .with_context(|| format!("write {}", checked.display()))?;
        return Ok(path);
    }

    let _ = fs::remove_file(&tmp);
    if path.is_file() {
        eprintln!("warning: refresh Flathub summary failed with {status}; using cached metadata");
        return Ok(path);
    }
    bail!("refresh Flathub summary failed with status {status}");
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
    let variant = variant_from_file(path, "(a(s(taya{sv}))a{sv})")
        .with_context(|| format!("parse Flathub summary {}", path.display()))?;
    let refs_v = variant.child_value(0);
    let mut refs = Vec::with_capacity(refs_v.n_children());

    for i in 0..refs_v.n_children() {
        let item = refs_v.child_value(i);
        let name = item.child_value(0).str().unwrap_or_default().to_string();
        let info = item.child_value(1);
        refs.push(RemoteRef {
            name,
            checksum: bytes_to_checksum(&info.child_value(1))?,
        });
    }

    Ok(refs)
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
    fs::create_dir_all(remote_dir).context("create remote metadata directory")?;
    let tmp = path.with_file_name(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("appstream.xml.gz"),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));

    let status = Command::new("fetch")
        .arg("-qo")
        .arg(&tmp)
        .arg(format!("{REMOTE}/appstream/{arch}/appstream.xml.gz"))
        .status()
        .with_context(|| format!("fetch Flathub AppStream metadata for {arch}"))?;
    if status.success() {
        fs::rename(&tmp, &path)
            .with_context(|| format!("move {} to {}", tmp.display(), path.display()))?;
        return Ok(path);
    }

    let _ = fs::remove_file(&tmp);
    bail!("refresh Flathub AppStream metadata for {arch} failed with status {status}");
}

fn decompress_appstream_xml(data: &[u8]) -> Result<String> {
    let payload = gzip_deflate_payload(data)?;
    let xml = decompress_to_vec(payload)
        .map_err(|error| anyhow::anyhow!("inflate AppStream gzip payload: {error:?}"))?;
    String::from_utf8(xml).context("AppStream XML is not UTF-8")
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
    let checksum = fetch_ref(ref_name)?;
    let objects = paths.objects();
    let commit = fetch_commit(&objects, &checksum)?;
    fs::create_dir_all(&dest).with_context(|| format!("create {}", dest.display()))?;

    let mut frontier = Vec::new();
    let mut file_groups: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    frontier.push((PathBuf::new(), commit.tree.clone()));

    let mut count_dirs = 0usize;

    while !frontier.is_empty() {
        eprintln!("fetching {} dirtree object(s)...", frontier.len());
        let batch = fetch_dirtree_batch(&objects, frontier)?;
        frontier = Vec::new();

        for (rel, tree) in batch {
            let target_dir = dest.join(&rel);
            fs::create_dir_all(&target_dir)
                .with_context(|| format!("create directory {}", target_dir.display()))?;
            count_dirs += 1;

            for file in tree.files {
                let target = target_dir.join(&file.name);
                file_groups.entry(file.checksum).or_default().push(target);
            }

            for dir in tree.dirs {
                frontier.push((rel.join(&dir.name), dir.tree));
            }
        }
    }

    let count_files: usize = file_groups.values().map(Vec::len).sum();
    let count_objects = file_groups.len();
    eprintln!("materializing {count_files} paths from {count_objects} unique file objects...");
    materialize_groups(&objects, file_groups)?;

    println!(
        "checked out {ref_name} to {} ({count_dirs} dirs, {count_files} files)",
        dest.display()
    );
    Ok(())
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

struct RuntimeRefParts {
    _name: String,
    arch: String,
    branch: String,
}

fn split_runtime_ref(runtime_ref: &str) -> Result<RuntimeRefParts> {
    let mut parts = runtime_ref.splitn(3, '/');
    let name = parts.next().context("missing runtime name")?;
    let arch = parts.next().context("missing runtime arch")?;
    let branch = parts.next().context("missing runtime branch")?;
    Ok(RuntimeRefParts {
        _name: name.to_string(),
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

struct Dirtree {
    files: Vec<FileEntry>,
    dirs: Vec<DirEntry>,
}

fn fetch_ref(ref_name: &str) -> Result<String> {
    let out = Command::new("fetch")
        .arg("-qo")
        .arg("-")
        .arg(format!("{REMOTE}/refs/heads/{ref_name}"))
        .output()
        .with_context(|| format!("fetch ref {ref_name}"))?;
    if !out.status.success() {
        bail!("fetch ref {ref_name} failed with status {}", out.status);
    }
    let checksum = String::from_utf8(out.stdout)?.trim().to_string();
    if checksum.len() != 64 {
        bail!("invalid checksum for {ref_name}: {checksum:?}");
    }
    Ok(checksum)
}

fn object_path(objects: &Path, checksum: &str, suffix: &str) -> PathBuf {
    objects
        .join(&checksum[0..2])
        .join(format!("{}.{}", &checksum[2..], suffix))
}

fn object_url(checksum: &str, suffix: &str) -> String {
    format!(
        "{REMOTE}/objects/{}/{}.{}",
        &checksum[0..2],
        &checksum[2..],
        suffix
    )
}

fn ensure_object(objects: &Path, checksum: &str, suffix: &str) -> Result<PathBuf> {
    let path = object_path(objects, checksum, suffix);
    if path.exists() {
        return Ok(path);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_file_name(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("object"),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    let status = Command::new("curl")
        .arg("--fail")
        .arg("--silent")
        .arg("--show-error")
        .arg("--location")
        .arg("--ipv4")
        .arg("--connect-timeout")
        .arg("30")
        .arg("--max-time")
        .arg("300")
        .arg("--retry")
        .arg("5")
        .arg("--retry-delay")
        .arg("1")
        .arg("--retry-all-errors")
        .arg("--output")
        .arg(&tmp)
        .arg(object_url(checksum, suffix))
        .status()
        .with_context(|| format!("fetch {checksum}.{suffix}"))?;
    if !status.success() {
        let _ = fs::remove_file(&tmp);
        bail!("fetch {checksum}.{suffix} failed with status {status}");
    }
    fs::rename(&tmp, &path)
        .with_context(|| format!("move {} to {}", tmp.display(), path.display()))?;
    Ok(path)
}

fn variant_from_file(path: &Path, ty: &'static str) -> Result<Variant> {
    let data = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let bytes = Bytes::from_owned(data);
    let ty = VariantTy::new(ty).context("invalid GVariant type")?;
    Ok(unsafe { Variant::from_bytes_with_type_trusted(&bytes, &ty) })
}

fn fetch_commit(objects: &Path, checksum: &str) -> Result<Commit> {
    let path = ensure_object(objects, checksum, "commit")?;
    let variant = variant_from_file(&path, "(a{sv}aya(say)sstayay)")?;

    let metadata_map = variant.child_value(0);
    let metadata = lookup_variant_string(&metadata_map, "xa.metadata").unwrap_or_default();
    let tree = checksum_from_child(&variant, 6)?;
    let dirmeta = checksum_from_child(&variant, 7)?;

    Ok(Commit {
        checksum: checksum.to_string(),
        tree,
        dirmeta,
        metadata,
    })
}

fn fetch_dirtree(objects: &Path, checksum: &str) -> Result<Dirtree> {
    let path = ensure_object(objects, checksum, "dirtree")?;
    let variant = variant_from_file(&path, "(a(say)a(sayay))")?;
    let files_v = variant.child_value(0);
    let dirs_v = variant.child_value(1);

    let mut files = Vec::new();
    for i in 0..files_v.n_children() {
        let item = files_v.child_value(i);
        files.push(FileEntry {
            name: item.child_value(0).str().unwrap_or_default().to_string(),
            checksum: bytes_to_checksum(&item.child_value(1))?,
        });
    }

    let mut dirs = Vec::new();
    for i in 0..dirs_v.n_children() {
        let item = dirs_v.child_value(i);
        dirs.push(DirEntry {
            name: item.child_value(0).str().unwrap_or_default().to_string(),
            tree: bytes_to_checksum(&item.child_value(1))?,
            _dirmeta: bytes_to_checksum(&item.child_value(2))?,
        });
    }

    Ok(Dirtree { files, dirs })
}

fn fetch_dirtree_batch(
    objects: &Path,
    tasks: Vec<(PathBuf, String)>,
) -> Result<Vec<(PathBuf, Dirtree)>> {
    let total = tasks.len();
    let queue = Arc::new(Mutex::new(VecDeque::from_iter(tasks)));
    let done = Arc::new(AtomicUsize::new(0));
    let results = Arc::new(Mutex::new(Vec::with_capacity(total)));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let workers = worker_count();

    std::thread::scope(|scope| {
        for _ in 0..workers {
            let objects = objects.to_path_buf();
            let queue = Arc::clone(&queue);
            let done = Arc::clone(&done);
            let results = Arc::clone(&results);
            let errors = Arc::clone(&errors);
            scope.spawn(move || loop {
                let task = queue.lock().unwrap().pop_front();
                let Some((rel, checksum)) = task else {
                    break;
                };

                match fetch_dirtree(&objects, &checksum) {
                    Ok(tree) => results.lock().unwrap().push((rel, tree)),
                    Err(error) => errors
                        .lock()
                        .unwrap()
                        .push(format!("{checksum}: {error:#}")),
                }

                let current = done.fetch_add(1, Ordering::Relaxed) + 1;
                if current % 500 == 0 || current == total {
                    eprintln!("fetched {current}/{total} dirtree objects...");
                }
            });
        }
    });

    let errors = errors.lock().unwrap();
    if !errors.is_empty() {
        bail!(
            "failed to fetch {} dirtree object(s):\n{}",
            errors.len(),
            errors.join("\n")
        );
    }
    drop(errors);

    let mut results = results.lock().unwrap();
    Ok(std::mem::take(&mut *results))
}

fn lookup_variant_string(map: &Variant, key: &str) -> Option<String> {
    for i in 0..map.n_children() {
        let entry = map.child_value(i);
        let key_variant = entry.child_value(0);
        let entry_key = key_variant.str()?;
        if entry_key != key {
            continue;
        }
        let boxed = entry.child_value(1);
        let value = boxed.as_variant()?;
        return value.str().map(ToString::to_string);
    }
    None
}

fn worker_count() -> usize {
    std::env::var("POC_FETCH_WORKERS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(6)
        .clamp(2, 12)
}

fn checksum_from_child(variant: &Variant, index: usize) -> Result<String> {
    bytes_to_checksum(&variant.child_value(index))
}

fn bytes_to_checksum(variant: &Variant) -> Result<String> {
    let bytes = variant.data_as_bytes();
    let data = bytes.as_ref();
    if data.len() != 32 {
        bail!("expected 32-byte checksum, got {}", data.len());
    }
    Ok(data.iter().map(|b| format!("{b:02x}")).collect())
}

fn materialize_groups(objects: &Path, file_groups: BTreeMap<String, Vec<PathBuf>>) -> Result<()> {
    let total = file_groups.len();
    let queue = Arc::new(Mutex::new(VecDeque::from_iter(file_groups)));
    let done = Arc::new(AtomicUsize::new(0));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let workers = worker_count();

    std::thread::scope(|scope| {
        for _ in 0..workers {
            let objects = objects.to_path_buf();
            let queue = Arc::clone(&queue);
            let done = Arc::clone(&done);
            let errors = Arc::clone(&errors);
            scope.spawn(move || loop {
                let task = queue.lock().unwrap().pop_front();
                let Some((checksum, targets)) = task else {
                    break;
                };
                if let Err(error) = materialize_file_object(&objects, &checksum, &targets) {
                    errors
                        .lock()
                        .unwrap()
                        .push(format!("{checksum}: {error:#}"));
                }
                let current = done.fetch_add(1, Ordering::Relaxed) + 1;
                if current % 500 == 0 || current == total {
                    eprintln!("materialized {current}/{total} file objects...");
                }
            });
        }
    });

    let errors = errors.lock().unwrap();
    if !errors.is_empty() {
        bail!(
            "failed to materialize {} file object(s):\n{}",
            errors.len(),
            errors.join("\n")
        );
    }
    Ok(())
}

fn materialize_file_object(objects: &Path, checksum: &str, targets: &[PathBuf]) -> Result<()> {
    let (mode, payload) = load_file_object(objects, checksum)?;
    for target in targets {
        write_file_payload(mode, &payload, target)
            .with_context(|| format!("checkout file {}", target.display()))?;
    }
    Ok(())
}

fn load_file_object(objects: &Path, checksum: &str) -> Result<(u32, Vec<u8>)> {
    let mut last_error = None;
    for attempt in 0..2 {
        let path = ensure_object(objects, checksum, "filez")?;
        let data =
            fs::read(&path).with_context(|| format!("read file object {}", path.display()))?;
        match decode_file_object(&data) {
            Ok(decoded) => return Ok(decoded),
            Err(error) if attempt == 0 => {
                last_error = Some(error);
                let _ = fs::remove_file(&path);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("unable to load file object")))
}

fn write_file_payload(mode: u32, payload: &[u8], target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }

    let kind = mode & 0o170000;
    match kind {
        0o100000 => {
            let mut file = fs::File::create(target)?;
            file.write_all(payload)?;
            file.flush()?;
            fs::set_permissions(target, fs::Permissions::from_mode((mode & 0o7777) as u32))?;
        }
        0o120000 => {
            let link_target =
                std::str::from_utf8(payload).context("symlink payload is not UTF-8")?;
            if fs::symlink_metadata(target).is_ok() {
                fs::remove_file(target)?;
            }
            unix_fs::symlink(link_target, target)?;
        }
        _ => bail!("unsupported file mode {mode:o} for {}", target.display()),
    }
    Ok(())
}

fn decode_file_object(data: &[u8]) -> Result<(u32, Vec<u8>)> {
    if data.len() < 28 {
        bail!("file object too small");
    }

    // OSTree archive-z2 file objects start with a short content metadata header.
    // The first word is the header size. For the Flathub objects observed here,
    // the big-endian mode is at bytes 24..28, followed shortly by raw deflate.
    let mode = u32::from_be_bytes(data[24..28].try_into().unwrap());
    let kind = mode & 0o170000;

    if kind == 0o120000 {
        if data.len() < 33 {
            bail!("symlink object too small");
        }
        let start = 32;
        let Some(end) = data[start..]
            .iter()
            .position(|b| *b == 0)
            .map(|p| start + p)
        else {
            bail!("symlink object has no target terminator");
        };
        return Ok((mode, data[start..end].to_vec()));
    }

    let expected_len = u32::from_be_bytes(data[12..16].try_into().unwrap()) as usize;
    for offset in 28..usize::min(256, data.len()) {
        if let Ok(payload) = decompress_to_vec(&data[offset..]) {
            if payload.len() == expected_len {
                return Ok((mode, payload));
            }
        }
        if let Ok(payload) = decompress_to_vec_zlib(&data[offset..]) {
            if payload.len() == expected_len {
                return Ok((mode, payload));
            }
        }
    }

    bail!("unable to inflate file object")
}

#[cfg(test)]
mod tests {
    use super::{
        parse_appstream_replacements, resolve_current_app_id_from_replacements, RemoteRef,
    };
    use std::collections::BTreeMap;

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
    fn current_app_id_follows_available_replacement() {
        let refs = vec![RemoteRef {
            name: "app/app.example.Current/x86_64/stable".to_string(),
            checksum: "app-2".to_string(),
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
            },
            RemoteRef {
                name: "app/app.example.Two/x86_64/stable".to_string(),
                checksum: "app-2".to_string(),
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
            },
            RemoteRef {
                name: "app/org.example.B/x86_64/stable".to_string(),
                checksum: "app-b".to_string(),
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
