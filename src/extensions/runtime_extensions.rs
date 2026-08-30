use super::{RuntimeGlExtension, RuntimeGtkThemeExtension, RuntimeVaapiExtension};
use crate::flatpak_metadata::{has_section, value};
use crate::installation::installation_paths::Installation;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub fn activate_default_gl_extension(
    paths: &Installation,
    runtime_ref: &str,
    runtime_dir: &Path,
) -> Result<Option<RuntimeGlExtension>> {
    let metadata_path = runtime_dir.join("metadata");
    let metadata = fs::read_to_string(&metadata_path)
        .with_context(|| format!("read runtime metadata {}", metadata_path.display()))?;
    let section = "Extension org.freedesktop.Platform.GL";
    if !has_section(&metadata, section) {
        return Ok(None);
    }

    let parts = split_runtime_ref(runtime_ref)?;
    let extension_branch = value(&metadata, section, "versions")
        .and_then(|versions| first_extension_version(&versions))
        .unwrap_or_else(|| parts.branch.clone());
    let directory = value(&metadata, section, "directory")
        .unwrap_or_else(|| "lib/x86_64-linux-gnu/GL".to_string());
    let runtime_mount_relative = PathBuf::from(directory).join("default");
    let runtime_mountpoint = runtime_dir.join("files").join(&runtime_mount_relative);
    validate_mountpoint("GL", &runtime_mountpoint)?;

    let ref_name = format!(
        "runtime/org.freedesktop.Platform.GL.default/{}/{}",
        parts.arch, extension_branch
    );
    let checkout_dir = paths.extensions().join(format!(
        "org.freedesktop.Platform.GL.default-{}",
        safe_dir_fragment(&extension_branch)
    ));
    validate_extension_checkout(&ref_name, &checkout_dir)?;

    Ok(Some(RuntimeGlExtension {
        ref_name,
        checkout_dir,
        runtime_mount_relative,
    }))
}

pub fn activate_intel_vaapi_extension(
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
    validate_mountpoint("VAAPI", &runtime_mountpoint)?;

    let ref_name = format!(
        "runtime/org.freedesktop.Platform.VAAPI.Intel/{}/{}",
        parts.arch, extension_branch
    );
    let checkout_dir = paths.extensions().join(format!(
        "org.freedesktop.Platform.VAAPI.Intel-{}",
        safe_dir_fragment(&extension_branch)
    ));
    validate_extension_checkout(&ref_name, &checkout_dir)?;

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

pub fn activate_gtk_theme_extension(
    paths: &Installation,
    runtime_ref: &str,
    runtime_dir: &Path,
    theme: Option<&str>,
) -> Result<Option<RuntimeGtkThemeExtension>> {
    let Some(theme) = theme.filter(|theme| valid_extension_suffix(theme)) else {
        return Ok(None);
    };
    let metadata_path = runtime_dir.join("metadata");
    let Ok(metadata) = fs::read_to_string(&metadata_path) else {
        return Ok(None);
    };
    let section = "Extension org.gtk.Gtk3theme";
    if !has_section(&metadata, section) {
        return Ok(None);
    }

    let Ok(parts) = split_runtime_ref(runtime_ref) else {
        return Ok(None);
    };
    let branch = value(&metadata, section, "version")
        .or_else(|| {
            value(&metadata, section, "versions")
                .and_then(|versions| first_extension_version(&versions))
        })
        .unwrap_or_else(|| parts.branch.clone());
    let name = format!("org.gtk.Gtk3theme.{theme}");
    let ref_name = format!("runtime/{name}/{}/{}", parts.arch, branch);
    let checkout_dir = paths.extensions().join(format!(
        "{}-{}",
        safe_dir_fragment(&name),
        safe_dir_fragment(&branch)
    ));
    if !checkout_dir.is_dir() {
        return Ok(None);
    }
    if validate_extension_checkout(&ref_name, &checkout_dir).is_err() {
        return Ok(None);
    }

    let directory = value(&metadata, section, "directory")
        .unwrap_or_else(|| "share/runtime/share/themes".to_string());
    if !valid_relative_extension_path(&directory) {
        return Ok(None);
    }
    let mut runtime_mount_relative = PathBuf::from(directory).join(theme);
    if let Some(suffix) =
        value(&metadata, section, "subdirectory-suffix").filter(|suffix| !suffix.is_empty())
    {
        if !valid_relative_extension_path(&suffix) {
            return Ok(None);
        }
        runtime_mount_relative.push(suffix);
    }
    let runtime_mountpoint = runtime_dir.join("files").join(&runtime_mount_relative);
    // Native Flatpak constructs extension destinations while assembling each
    // sandbox. nullfs needs the destination to exist in the runtime tree.
    if fs::create_dir_all(&runtime_mountpoint).is_err() {
        return Ok(None);
    }

    Ok(Some(RuntimeGtkThemeExtension {
        ref_name,
        checkout_dir,
        runtime_mount_relative,
    }))
}

pub(super) fn validate_extension_checkout(ref_name: &str, checkout_dir: &Path) -> Result<()> {
    let validation = || -> Result<()> {
        if !checkout_dir.is_dir() {
            bail!("checkout directory does not exist")
        }
        let marker_path = checkout_dir.join(".ostree-commit");
        let marker = fs::read_to_string(&marker_path)
            .with_context(|| format!("read deployment marker {}", marker_path.display()))?;
        let mut lines = marker.lines();
        let installed_ref = lines.next().context("deployment marker missing ref")?;
        if installed_ref != ref_name {
            bail!("deployment marker contains ref {installed_ref}, expected {ref_name}")
        }
        if lines.next().filter(|commit| !commit.is_empty()).is_none() {
            bail!("deployment marker missing commit")
        }
        lines
            .next()
            .context("deployment marker missing installed size")?
            .parse::<u64>()
            .context("deployment marker has invalid installed size")?;
        if lines.next().filter(|origin| !origin.is_empty()).is_none() {
            bail!("deployment marker missing origin")
        }
        let metadata_path = checkout_dir.join("metadata");
        let metadata = fs::read_to_string(&metadata_path)
            .with_context(|| format!("read extension metadata {}", metadata_path.display()))?;
        let expected_name = ref_name
            .strip_prefix("runtime/")
            .and_then(|value| value.split('/').next())
            .context("extension ref is not a runtime ref")?;
        let installed_name = value(&metadata, "Runtime", "name")
            .context("extension metadata is missing Runtime/name")?;
        if installed_name != expected_name {
            bail!("extension metadata contains name {installed_name}, expected {expected_name}")
        }
        let files_path = checkout_dir.join("files");
        if !files_path.is_dir() {
            bail!("files directory is missing: {}", files_path.display())
        }
        Ok(())
    };

    validation().with_context(|| {
        format!(
            "required extension {ref_name} is missing or corrupt at {}; run `flatpak update` or `flatpak repair`",
            checkout_dir.display()
        )
    })
}

fn validate_mountpoint(kind: &str, mountpoint: &Path) -> Result<()> {
    if !mountpoint.is_dir() {
        bail!(
            "required {kind} extension mountpoint is missing at {}; run `flatpak update` or `flatpak repair`",
            mountpoint.display()
        )
    }
    Ok(())
}

pub fn runtime_checkout_dir(runtime_ref: &str) -> String {
    let mut parts = runtime_ref.split('/');
    let name = parts.next().unwrap_or(runtime_ref);
    let _arch = parts.next();
    let branch = parts.next().unwrap_or("stable");
    format!("{name}-{}", branch.replace('/', "_"))
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

fn valid_relative_extension_path(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

pub(super) fn valid_extension_suffix(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
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

#[cfg(test)]
#[path = "tests/runtime_extensions.rs"]
mod tests;
