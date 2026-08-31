pub(crate) mod application_installation;
mod extra_data;
pub(crate) mod installation_paths;
pub(crate) mod startup_recovery;

mod application_records;
mod export_records;
mod generation_cleanup;
mod installed_sizes;
mod record_storage;
mod run_records;
mod runtime_records;

pub(crate) use crate::extensions::{
    activate_app_codec_extensions, activate_default_gl_extension, activate_gtk_theme_extension,
    activate_intel_vaapi_extension, activate_runtime_codec_extensions, reconcile_extensions,
    reconcile_extensions_with_metadata, required_extension_refs, runtime_checkout_dir,
    AppExtension, RuntimeCodecExtension, RuntimeGlExtension, RuntimeGtkThemeExtension,
    RuntimeVaapiExtension,
};
pub(crate) use crate::flatpak_metadata::value as metadata_value;
pub(crate) use crate::ostree::{prune_repo, recover_storage, remove_repo_refs, repair_repo};
pub(crate) use crate::sandbox::{resolve_app, FlatpakApp, ResolveAppOptions};
pub(crate) use application_installation::*;
pub(crate) use application_records::*;
pub(crate) use export_records::*;
pub(crate) use extra_data::apply_extra_data;
pub(crate) use generation_cleanup::*;
pub(crate) use installed_sizes::*;
pub(crate) use record_storage::ensure_layout;
pub(crate) use record_storage::write_atomic as write_state_atomic;
pub(crate) use run_records::*;
pub(crate) use runtime_records::*;

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub(crate) struct AppRecord {
    pub origin: String,
    pub runtime_origin: String,
    pub app_id: String,
    pub app_ref: String,
    pub app_commit: String,
    pub installed_size: u64,
    pub app_dir: PathBuf,
    pub arch: String,
    pub branch: String,
    pub runtime_ref: String,
    pub runtime_commit: String,
    pub runtime_dir: PathBuf,
    pub command: String,
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeRecord {
    pub origin: String,
    pub runtime_ref: String,
    pub runtime_commit: String,
    pub installed_size: u64,
    pub runtime_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct ExtensionRecord {
    pub origin: String,
    pub ref_name: String,
    pub commit: String,
    pub installed_size: u64,
    pub checkout_dir: PathBuf,
}
