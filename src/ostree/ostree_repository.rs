use crate::installation::installation_paths::Installation;
use anyhow::{Context, Result};
use glib::prelude::Cast;
use glib::translate::{from_glib_full, ToGlibPtr};
use glib::{Bytes, VariantDict};
use ostree::gio;
use ostree::gio::prelude::*;
use ostree::{ObjectType, Repo, RepoListObjectsFlags, RepoMode, RepoPruneFlags, RepoRemoteChange};
use std::fs::{self, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::PathBuf;

pub(super) const TRANSACTION_FILE: &str = ".storage-transaction";

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

        let storage = Self {
            repo,
            transaction_path: paths.data_root().join(TRANSACTION_FILE),
            _lock: lock,
        };
        storage.recover_activation()?;
        Ok(storage)
    }

    pub fn configure_remote(&self, remote: &crate::remotes::Remote) -> Result<()> {
        self.configure_remote_definition(remote)?;
        self.import_remote_gpg_key(remote)
    }

    pub(crate) fn configure_remote_definition(
        &self,
        remote: &crate::remotes::Remote,
    ) -> Result<()> {
        let options = VariantDict::new(None);
        options.insert("gpg-verify", remote.gpg_verify);
        options.insert("gpg-verify-summary", remote.gpg_verify);
        options.insert("http2", true);
        self.repo
            .remote_change(
                None::<&gio::File>,
                RepoRemoteChange::Replace,
                &remote.name,
                Some(&remote.url),
                Some(&options.end()),
                gio::Cancellable::NONE,
            )
            .with_context(|| format!("configure private OSTree remote {}", remote.name))
    }

    pub(crate) fn import_remote_gpg_key(&self, remote: &crate::remotes::Remote) -> Result<()> {
        if let Some(encoded) = &remote.gpg_key {
            let key = base64::decode(encoded.trim())
                .with_context(|| format!("decode GPG key for remote {}", remote.name))?;
            let key_bytes = Bytes::from_owned(key);
            let key_stream = gio::MemoryInputStream::from_bytes(&key_bytes);
            remote_gpg_import_all(&self.repo, &remote.name, &key_stream)
                .with_context(|| format!("import GPG key for remote {}", remote.name))?;
        }
        Ok(())
    }

    pub fn delete_remote(&self, name: &str) -> Result<()> {
        self.repo
            .remote_change(
                None::<&gio::File>,
                RepoRemoteChange::Delete,
                name,
                None,
                None,
                gio::Cancellable::NONE,
            )
            .with_context(|| format!("delete private OSTree remote {name}"))
    }

    pub fn verify_summary(&self, remote: &str, summary: &[u8], signatures: &[u8]) -> Result<()> {
        let summary = Bytes::from(summary);
        let signatures = Bytes::from(signatures);
        self.repo
            .verify_summary(remote, &summary, &signatures, gio::Cancellable::NONE)
            .with_context(|| format!("verify {remote} summary signature"))?
            .require_valid_signature()
            .with_context(|| format!("{remote} summary has no valid signature"))
    }

    pub fn fsck_commits(&self, commits: &[&str]) -> Result<()> {
        for commit in commits {
            self.repo
                .fsck_object(ObjectType::Commit, commit, gio::Cancellable::NONE)
                .with_context(|| format!("fsck commit {commit}"))?;
        }
        Ok(())
    }

    /// Return the logical checkout size used by upstream Flatpak deployment
    /// data: every regular file in the commit, rounded up to a 512-byte block.
    pub fn installed_size(&self, checksum: &str) -> Result<u64> {
        let (root, _) = self
            .repo
            .read_commit(checksum, gio::Cancellable::NONE)
            .with_context(|| format!("read OSTree commit {checksum}"))?;
        collect_installed_size(&root)
            .with_context(|| format!("collect installed size for commit {checksum}"))
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
        for remote in self.repo.remote_list() {
            for ref_name in refs {
                self.repo
                    .set_ref_immediate(
                        Some(remote.as_str()),
                        ref_name,
                        None,
                        gio::Cancellable::NONE,
                    )
                    .with_context(|| format!("remove OSTree ref {remote}:{ref_name}"))?;
            }
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

fn collect_installed_size(root: &gio::File) -> Result<u64> {
    const ATTRIBUTES: &str = "standard::name,standard::type,standard::size";
    let enumerator = root.enumerate_children(
        ATTRIBUTES,
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        gio::Cancellable::NONE,
    )?;
    let mut size = 0u64;
    while let Some(info) = enumerator.next_file(gio::Cancellable::NONE)? {
        match info.file_type() {
            gio::FileType::Regular => {
                let file_size = u64::try_from(info.size()).context("negative OSTree file size")?;
                size = size
                    .checked_add(file_size.saturating_add(511) / 512 * 512)
                    .context("installed size overflow")?;
            }
            gio::FileType::Directory => {
                size = size
                    .checked_add(collect_installed_size(&root.child(info.name()))?)
                    .context("installed size overflow")?;
            }
            _ => {}
        }
    }
    Ok(size)
}

fn remote_gpg_import_all(repo: &Repo, name: &str, stream: &gio::MemoryInputStream) -> Result<u32> {
    unsafe {
        let input: &gio::InputStream = stream.upcast_ref();
        let mut imported = 0;
        let mut error = std::ptr::null_mut();
        let ok = ostree::ffi::ostree_repo_remote_gpg_import(
            repo.to_glib_none().0,
            name.to_glib_none().0,
            input.to_glib_none().0,
            std::ptr::null(),
            &mut imported,
            std::ptr::null_mut(),
            &mut error,
        );
        if ok == glib::ffi::GFALSE {
            return Err(from_glib_full::<_, glib::Error>(error).into());
        }
        Ok(imported)
    }
}
