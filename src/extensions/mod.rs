mod application_extensions;
mod extension_points;
mod reconciliation;
mod runtime_extensions;

pub(crate) use application_extensions::activate_app_codec_extensions;
pub(crate) use extension_points::required_extension_refs;
pub(crate) use reconciliation::{reconcile_extensions, reconcile_extensions_with_metadata};
pub(crate) use runtime_extensions::{
    activate_default_gl_extension, activate_gtk_theme_extension, activate_intel_vaapi_extension,
    activate_runtime_codec_extensions, runtime_checkout_dir,
};

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub(crate) struct RuntimeGlExtension {
    pub ref_name: String,
    pub checkout_dir: PathBuf,
    pub runtime_mount_relative: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeVaapiExtension {
    pub ref_name: String,
    pub checkout_dir: PathBuf,
    pub runtime_mount_relative: PathBuf,
    pub ld_library_relative: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeGtkThemeExtension {
    pub ref_name: String,
    pub checkout_dir: PathBuf,
    pub runtime_mount_relative: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeCodecExtension {
    pub name: String,
    pub ref_name: String,
    pub checkout_dir: PathBuf,
    pub runtime_mount_relative: PathBuf,
    pub ld_library_relative: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub(crate) struct AppExtension {
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

impl RuntimeGtkThemeExtension {
    pub fn ref_name(&self) -> &str {
        &self.ref_name
    }
}

impl RuntimeCodecExtension {
    pub fn ref_name(&self) -> &str {
        &self.ref_name
    }
}
