use crate::installation::installation_paths::Installation;
use anyhow::{Context, Result};
use glib::{Bytes, VariantDict};
use ostree::gio;
use ostree::{ObjectType, Repo, RepoListObjectsFlags, RepoMode, RepoPruneFlags, RepoRemoteChange};
use std::fs::{self, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::PathBuf;

pub(super) const REMOTE_NAME: &str = "flathub";
const REMOTE_URL: &str = "https://dl.flathub.org/repo";
pub(super) const TRANSACTION_FILE: &str = ".storage-transaction";
const FLATHUB_GPG_KEY_BASE64: &str = include_str!("../../vendor/flathub.gpg.base64");
const FLATHUB_GPG_FINGERPRINT: &str = "6E5C05D979C76DAF93C081354184DD4D907A7CAE";

/// The only component that owns OSTree repository mechanics.  Ref selection,
/// deployment layout and user-facing policy remain in Rust.
pub struct Storage {
    pub(super) repo: Repo,
    pub(super) transaction_path: PathBuf,
    _lock: File,
}

pub(crate) fn repair_repo(paths: &Installation) -> Result<usize> {
    Storage::open(paths)?.fsck_all()
}

pub(crate) fn recover_storage(paths: &Installation) -> Result<()> {
    drop(Storage::open(paths)?);
    Ok(())
}

pub(crate) fn prune_repo(paths: &Installation) -> Result<(i32, i32, u64)> {
    Storage::open(paths)?.prune()
}

pub(crate) fn remove_repo_refs(paths: &Installation, refs: &[&str]) -> Result<()> {
    Storage::open(paths)?.remove_refs(refs)
}

impl Storage {
    pub fn open(paths: &Installation) -> Result<Self> {
        fs::create_dir_all(paths.data_root())
            .with_context(|| format!("create {}", paths.data_root().display()))?;

        let lock_path = paths.data_root().join(".storage.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("open {}", lock_path.display()))?;
        let rc = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("lock {}", lock_path.display()));
        }

        let repo_path = paths.repo();
        let repo = Repo::new_for_path(&repo_path);
        if repo_path.join("config").is_file() {
            repo.open(gio::Cancellable::NONE)
                .with_context(|| format!("open OSTree repository {}", repo_path.display()))?;
        } else {
            repo.create(RepoMode::BareUserOnly, gio::Cancellable::NONE)
                .with_context(|| format!("create OSTree repository {}", repo_path.display()))?;
        }

        let remote_options = VariantDict::new(None);
        remote_options.insert("gpg-verify", true);
        remote_options.insert("gpg-verify-summary", true);
        remote_options.insert("http2", true);
        repo.remote_change(
            None::<&gio::File>,
            RepoRemoteChange::Replace,
            REMOTE_NAME,
            Some(REMOTE_URL),
            Some(&remote_options.end()),
            gio::Cancellable::NONE,
        )
        .context("configure private Flathub remote")?;

        if !repo_path.join("flathub.trustedkeys.gpg").is_file() {
            let key = base64::decode(FLATHUB_GPG_KEY_BASE64.trim())
                .context("decode embedded Flathub signing key")?;
            let key_bytes = Bytes::from_owned(key);
            let key_stream = gio::MemoryInputStream::from_bytes(&key_bytes);
            repo.remote_gpg_import(
                REMOTE_NAME,
                Some(&key_stream),
                &[FLATHUB_GPG_FINGERPRINT],
                gio::Cancellable::NONE,
            )
            .context("import pinned Flathub signing key")?;
        }

        let storage = Self {
            repo,
            transaction_path: paths.data_root().join(TRANSACTION_FILE),
            _lock: lock,
        };
        storage.recover_activation()?;
        Ok(storage)
    }

    pub fn verify_summary(&self, summary: &[u8], signatures: &[u8]) -> Result<()> {
        let summary = Bytes::from(summary);
        let signatures = Bytes::from(signatures);
        self.repo
            .verify_summary(REMOTE_NAME, &summary, &signatures, gio::Cancellable::NONE)
            .context("verify Flathub summary signature")?
            .require_valid_signature()
            .context("Flathub summary has no valid signature")
    }

    pub fn fsck_commits(&self, commits: &[&str]) -> Result<()> {
        for commit in commits {
            self.repo
                .fsck_object(ObjectType::Commit, commit, gio::Cancellable::NONE)
                .with_context(|| format!("fsck commit {commit}"))?;
        }
        Ok(())
    }

    pub fn fsck_all(&self) -> Result<usize> {
        let objects = self
            .repo
            .list_objects(RepoListObjectsFlags::ALL.bits(), gio::Cancellable::NONE)
            .context("list OSTree objects")?;
        for object in objects.keys() {
            self.repo
                .fsck_object(
                    object.object_type(),
                    object.checksum(),
                    gio::Cancellable::NONE,
                )
                .with_context(|| format!("fsck {object}"))?;
        }
        Ok(objects.len())
    }

    pub fn remove_refs(&self, refs: &[&str]) -> Result<()> {
        for ref_name in refs {
            self.repo
                .set_ref_immediate(Some(REMOTE_NAME), ref_name, None, gio::Cancellable::NONE)
                .with_context(|| format!("remove OSTree ref {ref_name}"))?;
        }
        Ok(())
    }

    pub fn prune(&self) -> Result<(i32, i32, u64)> {
        self.repo
            .prune_static_deltas(None, gio::Cancellable::NONE)
            .context("prune unused static deltas")?;
        self.repo
            .prune(RepoPruneFlags::REFS_ONLY, 0, gio::Cancellable::NONE)
            .context("prune unreachable OSTree objects")
    }
}
