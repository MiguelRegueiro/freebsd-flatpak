use crate::paths::Installation;
use crate::remote::{load_arch_summary, ref_checksum, RemoteApp};
use crate::storage::{Deployment, Storage};
use anyhow::{bail, Context, Result};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

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
    let resolved_checksum;
    let checksum = match expected_checksum {
        Some(checksum) => checksum,
        None => {
            resolved_checksum = ref_checksum(&summary_path, ref_name)?;
            &resolved_checksum
        }
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

#[cfg(test)]
mod tests {
    use super::{generation_checkout_dir, required_extension_refs};
    use std::collections::BTreeSet;
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
}
