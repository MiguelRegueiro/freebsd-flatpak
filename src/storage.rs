use crate::paths::Installation;
use anyhow::{bail, Context, Result};
use glib::prelude::*;
use glib::{Bytes, VariantDict};
use ostree::gio;
use ostree::{
    AsyncProgress, ObjectType, Repo, RepoCheckoutAtOptions, RepoCheckoutMode,
    RepoCheckoutOverwriteMode, RepoListObjectsFlags, RepoMode, RepoPruneFlags, RepoRemoteChange,
};
use std::cell::Cell;
use std::fs::{self, File, OpenOptions};
use std::io::{IsTerminal, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

const REMOTE_NAME: &str = "flathub";
const REMOTE_URL: &str = "https://dl.flathub.org/repo";
const DEPLOYMENT_MARKER: &str = ".ostree-commit";
const TRANSACTION_FILE: &str = ".storage-transaction";
const FLATHUB_GPG_KEY_BASE64: &str = include_str!("../vendor/flathub.gpg.base64");
const FLATHUB_GPG_FINGERPRINT: &str = "6E5C05D979C76DAF93C081354184DD4D907A7CAE";

pub struct Deployment<'a> {
    pub kind: &'a str,
    pub ref_name: &'a str,
    pub checksum: &'a str,
    pub destination: &'a Path,
    pub force: bool,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct StorageTimings {
    pub pull: Duration,
    pub checkout: Duration,
}

struct Activation {
    destination: PathBuf,
    staging: PathBuf,
    backup: PathBuf,
}

/// The only component that owns OSTree repository mechanics.  Ref selection,
/// deployment layout and user-facing policy remain in Rust.
pub struct Storage {
    repo: Repo,
    transaction_path: PathBuf,
    _lock: File,
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

    pub fn deploy(&self, summary: &[u8], deployments: &[Deployment<'_>]) -> Result<StorageTimings> {
        let pending = deployments
            .iter()
            .filter(|deployment| {
                deployment.force || !checkout_matches(deployment.destination, deployment.checksum)
            })
            .collect::<Vec<_>>();

        for deployment in deployments {
            if !pending.iter().any(|item| std::ptr::eq(*item, deployment)) {
                println!("  Reusing {} {}", deployment.kind, deployment.ref_name);
            }
        }
        if pending.is_empty() {
            return Ok(StorageTimings::default());
        }

        println!("  Pulling {} ref(s) with libostree", pending.len());
        let pull = self.pull_exact(
            summary,
            &pending
                .iter()
                .map(|deployment| (deployment.ref_name, deployment.checksum))
                .collect::<Vec<_>>(),
        )?;

        let checkout_started = Instant::now();
        let mut activations = Vec::with_capacity(pending.len());
        for deployment in pending {
            println!("  Checking out {} {}", deployment.kind, deployment.ref_name);
            activations.push(self.stage_checkout(deployment)?);
        }

        self.write_transaction(&activations)?;
        self.finish_activation(&activations)?;
        Ok(StorageTimings {
            pull,
            checkout: checkout_started.elapsed(),
        })
    }

    pub fn checkout(
        &self,
        summary: &[u8],
        ref_name: &str,
        checksum: &str,
        destination: &Path,
    ) -> Result<()> {
        self.deploy(
            summary,
            &[Deployment {
                kind: "ref",
                ref_name,
                checksum,
                destination,
                force: true,
            }],
        )
        .map(|_| ())
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

    fn pull_exact(&self, summary: &[u8], refs: &[(&str, &str)]) -> Result<Duration> {
        let names = refs.iter().map(|(name, _)| *name).collect::<Vec<_>>();
        let commits = refs
            .iter()
            .map(|(_, checksum)| *checksum)
            .collect::<Vec<_>>();
        let options = VariantDict::new(None);
        options.insert_value("refs", &names.to_variant());
        options.insert_value("override-commit-ids", &commits.to_variant());
        options.insert("gpg-verify", true);
        // The indexed summary signature was checked before selecting the
        // architecture subsummary.  Its authenticated SHA-256 covers these
        // exact bytes, so asking libostree to verify a non-existent per-arch
        // signature would be both redundant and impossible.
        options.insert("gpg-verify-summary", false);
        options.insert_value("summary-bytes", &summary.to_variant());
        options.insert_value("summary-sig-bytes", &Vec::<u8>::new().to_variant());
        options.insert("depth", 0i32);
        options.insert("update-frequency", 200u32);
        options.insert("n-network-retries", 5u32);
        options.insert("retry-all-network-errors", true);
        options.insert("append-user-agent", "freebsd-flatpak/0.1");

        let progress = pull_progress();
        let started = Instant::now();
        let result = self.repo.pull_with_options(
            REMOTE_NAME,
            &options.end(),
            Some(&progress),
            gio::Cancellable::NONE,
        );
        progress.finish();
        if std::io::stdout().is_terminal() {
            print!("\r\x1b[2K");
        }
        result.context("pull verified Flathub refs with libostree")?;
        let elapsed = started.elapsed();
        println!("    Pull completed in {:.1}s", elapsed.as_secs_f64());

        self.fsck_commits(&commits)?;
        Ok(elapsed)
    }

    fn stage_checkout(&self, deployment: &Deployment<'_>) -> Result<Activation> {
        let name = deployment
            .destination
            .file_name()
            .and_then(|name| name.to_str())
            .context("checkout destination has no UTF-8 file name")?;
        let short_commit = &deployment.checksum[..deployment.checksum.len().min(12)];
        let staging = deployment
            .destination
            .with_file_name(format!(".{name}.ostree-{short_commit}.staging"));
        let backup = deployment
            .destination
            .with_file_name(format!(".{name}.ostree-previous"));

        remove_path_if_exists(&staging)?;
        fs::create_dir_all(
            staging
                .parent()
                .context("checkout destination has no parent")?,
        )?;
        let checkout_options = RepoCheckoutAtOptions {
            mode: RepoCheckoutMode::User,
            overwrite_mode: RepoCheckoutOverwriteMode::None,
            enable_fsync: true,
            bareuseronly_dirs: true,
            ..Default::default()
        };
        if let Err(error) = self.repo.checkout_at(
            Some(&checkout_options),
            libc::AT_FDCWD,
            &staging,
            deployment.checksum,
            gio::Cancellable::NONE,
        ) {
            let _ = remove_path_if_exists(&staging);
            return Err(error).with_context(|| {
                format!(
                    "checkout {} at {}",
                    deployment.ref_name,
                    deployment.destination.display()
                )
            });
        }
        validate_checkout(&staging)?;
        let marker_path = staging.join(DEPLOYMENT_MARKER);
        let mut marker = File::create(&marker_path)
            .with_context(|| format!("create {}", marker_path.display()))?;
        writeln!(marker, "{}", deployment.ref_name)?;
        writeln!(marker, "{}", deployment.checksum)?;
        marker.sync_all()?;

        Ok(Activation {
            destination: deployment.destination.to_path_buf(),
            staging,
            backup,
        })
    }

    fn write_transaction(&self, activations: &[Activation]) -> Result<()> {
        let partial = self.transaction_path.with_extension("transaction.part");
        let mut file =
            File::create(&partial).with_context(|| format!("create {}", partial.display()))?;
        writeln!(file, "1")?;
        for activation in activations {
            writeln!(
                file,
                "{}\t{}\t{}",
                activation.destination.display(),
                activation.staging.display(),
                activation.backup.display()
            )?;
        }
        file.sync_all()?;
        fs::rename(&partial, &self.transaction_path)
            .with_context(|| format!("publish {}", self.transaction_path.display()))?;
        sync_parent(&self.transaction_path)?;
        Ok(())
    }

    fn finish_activation(&self, activations: &[Activation]) -> Result<()> {
        for activation in activations {
            remove_path_if_exists(&activation.backup)?;
            if activation.destination.exists() {
                fs::rename(&activation.destination, &activation.backup).with_context(|| {
                    format!(
                        "preserve current deployment {}",
                        activation.destination.display()
                    )
                })?;
            }
            fs::rename(&activation.staging, &activation.destination).with_context(|| {
                format!("activate deployment {}", activation.destination.display())
            })?;
            sync_parent(&activation.destination)?;
        }

        for activation in activations {
            remove_path_if_exists(&activation.backup)?;
        }
        fs::remove_file(&self.transaction_path)
            .with_context(|| format!("complete {}", self.transaction_path.display()))?;
        sync_parent(&self.transaction_path)?;
        Ok(())
    }

    fn recover_activation(&self) -> Result<()> {
        recover_activation_file(&self.transaction_path)
    }
}

fn recover_activation_file(transaction_path: &Path) -> Result<()> {
    if !transaction_path.is_file() {
        return Ok(());
    }
    eprintln!("Recovering interrupted OSTree deployment transaction");
    let transaction = fs::read_to_string(transaction_path)
        .with_context(|| format!("read {}", transaction_path.display()))?;
    let mut lines = transaction.lines();
    if lines.next() != Some("1") {
        bail!("unsupported storage transaction format");
    }
    let mut activations = Vec::new();
    for line in lines {
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 3 {
            bail!("invalid storage transaction entry");
        }
        activations.push(Activation {
            destination: PathBuf::from(fields[0]),
            staging: PathBuf::from(fields[1]),
            backup: PathBuf::from(fields[2]),
        });
    }

    for activation in &activations {
        if activation.staging.exists() {
            if activation.destination.exists() {
                remove_path_if_exists(&activation.backup)?;
                fs::rename(&activation.destination, &activation.backup).with_context(|| {
                    format!(
                        "preserve deployment while resuming {}",
                        activation.destination.display()
                    )
                })?;
            }
            fs::rename(&activation.staging, &activation.destination).with_context(|| {
                format!("resume deployment {}", activation.destination.display())
            })?;
            sync_parent(&activation.destination)?;
        } else if activation.destination.exists() {
            // This activation completed before the interruption.
        } else if activation.backup.exists() {
            fs::rename(&activation.backup, &activation.destination).with_context(|| {
                format!("restore deployment {}", activation.destination.display())
            })?;
            bail!("deployment transaction was incomplete; restored previous deployment");
        } else {
            bail!(
                "cannot recover deployment {}; staging and backup are missing",
                activation.destination.display()
            );
        }
    }
    for activation in &activations {
        remove_path_if_exists(&activation.backup)?;
    }
    fs::remove_file(transaction_path)?;
    sync_parent(transaction_path)?;
    Ok(())
}

fn pull_progress() -> AsyncProgress {
    let progress = AsyncProgress::new();
    let last_bytes = Rc::new(Cell::new(0u64));
    let last_bytes_changed = Rc::clone(&last_bytes);
    let terminal = std::io::stdout().is_terminal();
    progress.connect_changed(move |progress| {
        let bytes = progress.uint64("bytes-transferred");
        if !terminal && bytes.saturating_sub(last_bytes_changed.get()) < 16 * 1024 * 1024 {
            return;
        }
        last_bytes_changed.set(bytes);
        let fetched_parts = progress.uint("fetched-delta-parts");
        let total_parts = progress.uint("total-delta-parts");
        let total_size = progress.uint64("total-delta-part-size");
        let line = if total_parts > 0 {
            format!(
                "    Static delta {fetched_parts}/{total_parts}: {} / {}",
                format_bytes(bytes),
                format_bytes(total_size)
            )
        } else {
            format!("    Received {}", format_bytes(bytes))
        };
        if terminal {
            print!("\r\x1b[2K{line}");
            let _ = std::io::stdout().flush();
        } else {
            println!("{line}");
        }
    });
    progress
}

fn checkout_matches(destination: &Path, checksum: &str) -> bool {
    if validate_checkout(destination).is_err() {
        return false;
    }
    fs::read_to_string(destination.join(DEPLOYMENT_MARKER))
        .ok()
        .and_then(|marker| marker.lines().nth(1).map(ToOwned::to_owned))
        .as_deref()
        == Some(checksum)
}

fn validate_checkout(path: &Path) -> Result<()> {
    if !path.join("metadata").is_file() || !path.join("files").is_dir() {
        bail!("incomplete OSTree checkout at {}", path.display());
    }
    Ok(())
}

fn remove_path_if_exists(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            fs::remove_dir_all(path).with_context(|| format!("remove directory {}", path.display()))
        }
        Ok(_) => fs::remove_file(path).with_context(|| format!("remove file {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

fn sync_parent(path: &Path) -> Result<()> {
    let parent = path.parent().context("path has no parent")?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("sync {}", parent.display()))
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);

    fn test_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "freebsd-flatpak-storage-test-{}-{}",
            std::process::id(),
            NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn deployment(path: &Path, value: &str) {
        fs::create_dir_all(path).unwrap();
        fs::write(path.join("value"), value).unwrap();
    }

    #[test]
    fn recovery_finishes_every_staged_activation() {
        let root = test_dir();
        fs::create_dir_all(&root).unwrap();
        let transaction = root.join(TRANSACTION_FILE);

        let first = root.join("first");
        let first_staging = root.join(".first.staging");
        let first_backup = root.join(".first.previous");
        deployment(&first, "new-first");
        deployment(&first_backup, "old-first");

        let second = root.join("second");
        let second_staging = root.join(".second.staging");
        let second_backup = root.join(".second.previous");
        deployment(&second, "old-second");
        deployment(&second_staging, "new-second");

        fs::write(
            &transaction,
            format!(
                "1\n{}\t{}\t{}\n{}\t{}\t{}\n",
                first.display(),
                first_staging.display(),
                first_backup.display(),
                second.display(),
                second_staging.display(),
                second_backup.display()
            ),
        )
        .unwrap();

        recover_activation_file(&transaction).unwrap();

        assert_eq!(
            fs::read_to_string(first.join("value")).unwrap(),
            "new-first"
        );
        assert_eq!(
            fs::read_to_string(second.join("value")).unwrap(),
            "new-second"
        );
        assert!(!first_backup.exists());
        assert!(!second_backup.exists());
        assert!(!transaction.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
