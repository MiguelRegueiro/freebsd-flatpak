use super::{RuntimeGlExtension, RuntimeVaapiExtension};
use crate::flatpak_metadata::{has_section, value};
use crate::installation::installation_paths::Installation;
use crate::ostree::{Deployment, Storage, StorageTimings};
use crate::remotes::{load_arch_summary, ref_checksum};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub fn ensure_default_gl_extension(
    paths: &Installation,
    runtime_ref: &str,
    runtime_dir: &Path,
) -> Result<Option<RuntimeGlExtension>> {
    Ok(ensure_default_gl_extension_timed(paths, runtime_ref, runtime_dir)?.0)
}

pub(crate) fn ensure_default_gl_extension_timed(
    paths: &Installation,
    runtime_ref: &str,
    runtime_dir: &Path,
) -> Result<(Option<RuntimeGlExtension>, StorageTimings)> {
    let metadata_path = runtime_dir.join("metadata");
    let metadata = fs::read_to_string(&metadata_path)
        .with_context(|| format!("read runtime metadata {}", metadata_path.display()))?;
    let section = "Extension org.freedesktop.Platform.GL";
    if !has_section(&metadata, section) {
        return Ok((None, Default::default()));
    }

    let parts = split_runtime_ref(runtime_ref)?;
    let extension_branch = value(&metadata, section, "versions")
        .and_then(|versions| first_extension_version(&versions))
        .unwrap_or_else(|| parts.branch.clone());
    let directory = value(&metadata, section, "directory")
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
    let checkout_dir = extension_checkout_dir(paths, &ref_name)?;
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
    if !has_section(&metadata, section) {
        return Ok(None);
    }

    let parts = split_runtime_ref(runtime_ref)?;
    let extension_branch = value(&metadata, section, "version")
        .or_else(|| {
            value(&metadata, section, "versions")
                .and_then(|versions| first_extension_version(&versions))
        })
        .unwrap_or_else(|| parts.branch.clone());
    let directory = value(&metadata, section, "directory")
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
    let checkout_dir = extension_checkout_dir(paths, &ref_name)?;
    checkout_if_missing(paths, "extension", &ref_name, None, &checkout_dir, false)?;

    let ld_library_relative = value(&metadata, section, "add-ld-path")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);

    Ok(Some(RuntimeVaapiExtension {
        ref_name,
        checkout_dir,
        runtime_mount_relative,
        ld_library_relative,
    }))
}

pub(super) fn checkout_if_missing(
    paths: &Installation,
    kind: &str,
    ref_name: &str,
    expected_checksum: Option<&str>,
    dest: &Path,
    force: bool,
) -> Result<StorageTimings> {
    let mut remotes = crate::remotes::enabled_remotes(paths)?;
    remotes.sort_by_key(|remote| {
        (
            remote.name != crate::remotes::DEFAULT_REMOTE,
            remote.name.clone(),
        )
    });
    for remote in remotes {
        let (_, summary_path, _) = load_arch_summary(paths, &remote)?;
        let resolved_checksum = match expected_checksum {
            Some(checksum) => checksum.to_string(),
            None => match ref_checksum(&summary_path, ref_name) {
                Ok(checksum) => checksum,
                Err(_) => continue,
            },
        };
        let summary =
            fs::read(&summary_path).with_context(|| format!("read {}", summary_path.display()))?;
        return Storage::open(paths)?.deploy(
            &summary,
            &[Deployment {
                remote: &remote.name,
                kind,
                ref_name,
                checksum: &resolved_checksum,
                destination: dest,
                force,
            }],
        );
    }
    anyhow::bail!("ref is not present in an enabled remote: {ref_name}")
}

pub fn runtime_checkout_dir(runtime_ref: &str) -> String {
    let mut parts = runtime_ref.split('/');
    let name = parts.next().unwrap_or(runtime_ref);
    let _arch = parts.next();
    let branch = parts.next().unwrap_or("stable");
    format!("{name}-{}", branch.replace('/', "_"))
}

pub fn extension_checkout_dir(paths: &Installation, ref_name: &str) -> Result<PathBuf> {
    let parts = parse_runtime_ref(ref_name).context("extension ref must be a runtime ref")?;
    Ok(paths.extensions().join(format!(
        "{}-{}-{}",
        safe_dir_fragment(&parts.name),
        safe_dir_fragment(&parts.arch),
        safe_dir_fragment(&parts.branch)
    )))
}

pub(super) fn parse_runtime_ref(ref_name: &str) -> Option<RuntimeRefParts> {
    let runtime_ref = ref_name.strip_prefix("runtime/")?;
    split_runtime_ref(runtime_ref).ok()
}

pub(super) struct RuntimeRefParts {
    pub(super) name: String,
    pub(super) arch: String,
    pub(super) branch: String,
}

pub(super) fn split_runtime_ref(runtime_ref: &str) -> Result<RuntimeRefParts> {
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

pub(super) fn first_extension_version(versions: &str) -> Option<String> {
    versions
        .split(';')
        .map(str::trim)
        .find(|version| !version.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn safe_dir_fragment(value: &str) -> String {
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
