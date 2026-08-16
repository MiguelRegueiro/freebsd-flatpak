use anyhow::{bail, Context, Result};
use glib::{Bytes, Variant, VariantTy};
use miniz_oxide::inflate::decompress_to_vec;
use miniz_oxide::inflate::decompress_to_vec_zlib;
use std::collections::{BTreeMap, VecDeque};
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

pub fn inspect_refs(refs: &[String]) -> Result<()> {
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
        let commit = fetch_commit(&checksum)?;
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

pub fn install_app(project_root: &Path, app_id: &str) -> Result<InstalledApp> {
    let remote = resolve_remote_app(app_id)?;
    checkout_remote_app(project_root, &remote, false, false)
}

pub fn update_app(
    project_root: &Path,
    remote: &RemoteApp,
    force_app: bool,
    force_runtime: bool,
) -> Result<InstalledApp> {
    checkout_remote_app(project_root, remote, force_app, force_runtime)
}

pub fn resolve_remote_app(app_id: &str) -> Result<RemoteApp> {
    if app_id.contains('/') {
        bail!("app id must not contain '/': {app_id}");
    }

    let arch = host_flatpak_arch()?;
    let summary_path = fetch_summary()?;
    let refs = parse_summary_refs(&summary_path)?;
    let app_remote_ref = choose_app_ref(&refs, app_id, &arch)?;
    let app_commit = fetch_commit(&app_remote_ref.checksum)?;
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
    let runtime_commit = fetch_commit(&runtime_remote_ref.checksum)?;

    let app_ref_parts = split_flatpak_ref(&app_remote_ref.name)?;
    Ok(RemoteApp {
        app_id: app_id.to_string(),
        app_ref: app_remote_ref.name,
        app_commit: app_remote_ref.checksum,
        arch,
        branch: app_ref_parts.branch,
        runtime_ref,
        runtime_commit: runtime_commit.checksum,
        command,
    })
}

pub fn search_apps(query: &str) -> Result<Vec<SearchResult>> {
    let query = query.to_ascii_lowercase();
    let arch = host_flatpak_arch()?;
    let summary_path = fetch_summary()?;
    let refs = parse_summary_refs(&summary_path)?;
    let mut results = Vec::new();

    for remote_ref in refs {
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
    project_root: &Path,
    remote: &RemoteApp,
    force_app: bool,
    force_runtime: bool,
) -> Result<InstalledApp> {
    let app_dir = project_root
        .join("runtime")
        .join("app")
        .join(&remote.app_id);
    let runtime_dir = project_root
        .join("runtime")
        .join(runtime_checkout_dir(&remote.runtime_ref));

    checkout_if_missing(&remote.app_ref, &app_dir, force_app)?;
    checkout_if_missing(
        &format!("runtime/{}", remote.runtime_ref),
        &runtime_dir,
        force_runtime,
    )?;

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

pub fn resolve_app(
    project_root: &Path,
    app_id: &str,
    options: ResolveAppOptions,
) -> Result<FlatpakApp> {
    if app_id.contains('/') {
        bail!("app id must not contain '/': {app_id}");
    }

    let app_dir = options
        .app_dir
        .unwrap_or_else(|| project_root.join("runtime").join("app").join(app_id));
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

    let runtime_dir = options.runtime_dir.unwrap_or_else(|| {
        project_root
            .join("runtime")
            .join(runtime_checkout_dir(&runtime_ref))
    });

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

fn checkout_if_missing(ref_name: &str, dest: &Path, force: bool) -> Result<()> {
    if !force && dest.join("metadata").is_file() && dest.join("files").is_dir() {
        println!("reusing checkout for {ref_name}: {}", dest.display());
        return Ok(());
    }
    if force && dest.exists() {
        fs::remove_dir_all(dest)
            .with_context(|| format!("remove old checkout {}", dest.display()))?;
    }
    checkout_ref(ref_name, dest.to_path_buf())
}

fn fetch_summary() -> Result<PathBuf> {
    let path = PathBuf::from("downloads").join("summary");
    fs::create_dir_all("downloads").context("create downloads directory")?;
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
        return Ok(path);
    }

    let _ = fs::remove_file(&tmp);
    if path.exists() {
        eprintln!(
            "warning: could not refresh Flathub summary, using cached {}",
            path.display()
        );
        return Ok(path);
    }

    bail!("fetch Flathub summary failed with status {status}");
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

pub fn checkout_ref(ref_name: &str, dest: PathBuf) -> Result<()> {
    let checksum = fetch_ref(ref_name)?;
    let commit = fetch_commit(&checksum)?;
    fs::create_dir_all(&dest).with_context(|| format!("create {}", dest.display()))?;

    let mut frontier = Vec::new();
    let mut file_groups: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    frontier.push((PathBuf::new(), commit.tree.clone()));

    let mut count_dirs = 0usize;

    while !frontier.is_empty() {
        eprintln!("fetching {} dirtree object(s)...", frontier.len());
        let batch = fetch_dirtree_batch(frontier)?;
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
    materialize_groups(file_groups)?;

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

pub fn runtime_checkout_dir(runtime_ref: &str) -> String {
    let mut parts = runtime_ref.split('/');
    let name = parts.next().unwrap_or(runtime_ref);
    let _arch = parts.next();
    let branch = parts.next().unwrap_or("stable");
    format!("{name}-{}", branch.replace('/', "_"))
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

fn object_path(checksum: &str, suffix: &str) -> PathBuf {
    PathBuf::from("downloads")
        .join("objects")
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

fn ensure_object(checksum: &str, suffix: &str) -> Result<PathBuf> {
    let path = object_path(checksum, suffix);
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

fn fetch_commit(checksum: &str) -> Result<Commit> {
    let path = ensure_object(checksum, "commit")?;
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

fn fetch_dirtree(checksum: &str) -> Result<Dirtree> {
    let path = ensure_object(checksum, "dirtree")?;
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

fn fetch_dirtree_batch(tasks: Vec<(PathBuf, String)>) -> Result<Vec<(PathBuf, Dirtree)>> {
    let total = tasks.len();
    let queue = Arc::new(Mutex::new(VecDeque::from_iter(tasks)));
    let done = Arc::new(AtomicUsize::new(0));
    let results = Arc::new(Mutex::new(Vec::with_capacity(total)));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let workers = worker_count();

    std::thread::scope(|scope| {
        for _ in 0..workers {
            let queue = Arc::clone(&queue);
            let done = Arc::clone(&done);
            let results = Arc::clone(&results);
            let errors = Arc::clone(&errors);
            scope.spawn(move || loop {
                let task = queue.lock().unwrap().pop_front();
                let Some((rel, checksum)) = task else {
                    break;
                };

                match fetch_dirtree(&checksum) {
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

fn materialize_groups(file_groups: BTreeMap<String, Vec<PathBuf>>) -> Result<()> {
    let total = file_groups.len();
    let queue = Arc::new(Mutex::new(VecDeque::from_iter(file_groups)));
    let done = Arc::new(AtomicUsize::new(0));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let workers = worker_count();

    std::thread::scope(|scope| {
        for _ in 0..workers {
            let queue = Arc::clone(&queue);
            let done = Arc::clone(&done);
            let errors = Arc::clone(&errors);
            scope.spawn(move || loop {
                let task = queue.lock().unwrap().pop_front();
                let Some((checksum, targets)) = task else {
                    break;
                };
                if let Err(error) = materialize_file_object(&checksum, &targets) {
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

fn materialize_file_object(checksum: &str, targets: &[PathBuf]) -> Result<()> {
    let (mode, payload) = load_file_object(checksum)?;
    for target in targets {
        write_file_payload(mode, &payload, target)
            .with_context(|| format!("checkout file {}", target.display()))?;
    }
    Ok(())
}

fn load_file_object(checksum: &str) -> Result<(u32, Vec<u8>)> {
    let mut last_error = None;
    for attempt in 0..2 {
        let path = ensure_object(checksum, "filez")?;
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
