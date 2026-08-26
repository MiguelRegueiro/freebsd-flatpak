mod appstream_metadata;
mod metadata_cache;
mod ostree_summary;
mod ref_resolution;
mod remote_config;

use std::sync::atomic::{AtomicU64, Ordering};

use std::path::PathBuf;

pub(crate) use metadata_cache::load_arch_summary;
pub(crate) use ostree_summary::ref_checksum;
pub use ref_resolution::{
    checkout_ref, inspect_refs, load_remote_metadata, resolve_remote_app, resolve_remote_runtime,
    search_apps,
};
pub(crate) use remote_config::{
    add as add_remote, delete as delete_remote, from_location, initialize, modify as modify_remote,
    read_gpg_key, Remote, DEFAULT_REMOTE,
};
pub use remote_config::{enabled as enabled_remotes, get as get_remote, list as list_remotes};

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub remote: String,
    pub app_id: String,
    pub app_ref: String,
    pub arch: String,
    pub branch: String,
}

#[derive(Debug, Clone)]
pub struct RemoteRefInfo {
    pub remote: String,
    pub ref_name: String,
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
    pub origin: String,
    pub runtime_origin: String,
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
pub struct RemoteRuntime {
    pub origin: String,
    pub ref_name: String,
    pub runtime_ref: String,
    pub commit: String,
    pub is_extension: bool,
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
    remote: Remote,
    arch: String,
    refs: Vec<RemoteRef>,
    remote_dir: PathBuf,
    summary_path: PathBuf,
    collection_id: Option<String>,
}

static UNIQUE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn unique_sequence() -> u64 {
    UNIQUE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
}

fn trace_resolution(label: &str, started: std::time::Instant) {
    if std::env::var_os("FREEBSD_FLATPAK_TRACE_RESOLUTION").is_some() {
        eprintln!(
            "resolution timing: {label}: {:.3}s",
            started.elapsed().as_secs_f64()
        );
    }
}
