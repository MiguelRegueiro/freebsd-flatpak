mod checkout_activation;
mod commit_history;
mod ostree_repository;

pub(crate) use ostree_repository::{
    prune_repo, recover_storage, remove_remote_refs, repair_repo, Storage,
};

use std::path::Path;
use std::time::Duration;

pub(crate) struct Deployment<'a> {
    pub remote: &'a str,
    pub kind: &'a str,
    pub ref_name: &'a str,
    pub checksum: &'a str,
    pub destination: &'a Path,
    pub force: bool,
}

pub(crate) struct RemoteSource<'a> {
    pub name: &'a str,
    pub summary: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitInfo {
    pub checksum: String,
    pub parent: Option<String>,
    pub timestamp: u64,
    pub subject: String,
    pub body: String,
    pub flatpak_metadata: Option<String>,
    pub version: Option<String>,
    pub collection_id: Option<String>,
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct StorageTimings {
    pub pull: Duration,
    pub checkout: Duration,
}
