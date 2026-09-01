use super::installation_paths::Installation;
use crate::extensions::runtime_checkout_dir;
use crate::ostree::{remove_remote_refs, Deployment, RemoteSource, Storage};
use crate::remotes::{load_arch_summary, RemoteApp, RemoteRuntime};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct InstalledApp {
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
    pub runtime_installed_size: u64,
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
    let existing_runtime = super::get_runtime(paths, &remote.runtime_ref)?;
    let runtime_dir = if !force_runtime
        && existing_runtime
            .as_ref()
            .is_some_and(|record| record.runtime_commit == remote.runtime_commit)
    {
        paths.absolute_data_path(&existing_runtime.unwrap().runtime_dir)
    } else {
        generation_checkout_dir(
            &paths
                .runtimes()
                .join(runtime_checkout_dir(&remote.runtime_ref)),
            &remote.runtime_commit,
            force_runtime,
        )
    };
    let configured = crate::remotes::get_remote(paths, &remote.origin)?;
    let (_, summary_path, _) = load_arch_summary(paths, &configured)?;
    let summary =
        fs::read(&summary_path).with_context(|| format!("read {}", summary_path.display()))?;
    let runtime_full_ref = format!("runtime/{}", remote.runtime_ref);
    let storage = Storage::open(paths)?;
    let timings = if remote.origin == remote.runtime_origin {
        storage.deploy(
            &summary,
            &[
                Deployment {
                    remote: &remote.origin,
                    kind: "application",
                    ref_name: &remote.app_ref,
                    checksum: &remote.app_commit,
                    destination: &app_dir,
                    force: force_app,
                },
                Deployment {
                    remote: &remote.runtime_origin,
                    kind: "runtime",
                    ref_name: &runtime_full_ref,
                    checksum: &remote.runtime_commit,
                    destination: &runtime_dir,
                    force: force_runtime,
                },
            ],
        )?
    } else {
        let runtime_remote = crate::remotes::get_remote(paths, &remote.runtime_origin)?;
        let (_, runtime_summary_path, _) = load_arch_summary(paths, &runtime_remote)?;
        let runtime_summary = fs::read(&runtime_summary_path)
            .with_context(|| format!("read {}", runtime_summary_path.display()))?;
        storage.deploy_from_sources(
            &[
                RemoteSource {
                    name: &remote.origin,
                    summary: &summary,
                },
                RemoteSource {
                    name: &remote.runtime_origin,
                    summary: &runtime_summary,
                },
            ],
            &[
                Deployment {
                    remote: &remote.origin,
                    kind: "application",
                    ref_name: &remote.app_ref,
                    checksum: &remote.app_commit,
                    destination: &app_dir,
                    force: force_app,
                },
                Deployment {
                    remote: &remote.runtime_origin,
                    kind: "runtime",
                    ref_name: &runtime_full_ref,
                    checksum: &remote.runtime_commit,
                    destination: &runtime_dir,
                    force: force_runtime,
                },
            ],
        )?
    };
    let installed_size = storage.installed_size(&remote.app_commit)?;
    let runtime_installed_size = storage.installed_size(&remote.runtime_commit)?;
    drop(storage);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pin_id = format!("install-{nonce:x}-{}", std::process::id());
    let pin_root = paths.chroots().join(&remote.app_id).join(&pin_id);
    let pin = super::write_checkout_pin(
        paths,
        &remote.app_id,
        &pin_id,
        &pin_root,
        &app_dir,
        &runtime_dir,
    )?;
    let extra_result = super::apply_extra_data(paths, &app_dir, &runtime_dir);
    let unpin_result = super::remove_run_record(&pin);
    extra_result?;
    unpin_result?;
    Ok(InstalledApp {
        origin: remote.origin.clone(),
        runtime_origin: remote.runtime_origin.clone(),
        app_id: remote.app_id.clone(),
        app_ref: remote.app_ref.clone(),
        app_commit: remote.app_commit.clone(),
        installed_size,
        app_dir,
        arch: remote.arch.clone(),
        branch: remote.branch.clone(),
        runtime_ref: remote.runtime_ref.clone(),
        runtime_commit: remote.runtime_commit.clone(),
        runtime_installed_size,
        runtime_dir,
        command: remote.command.clone(),
        timings: InstallTimings {
            resolution: Duration::ZERO,
            pull: timings.pull,
            checkout: timings.checkout,
        },
    })
}

pub fn update_runtime(
    paths: &Installation,
    remote: &RemoteRuntime,
    force: bool,
    explicitly_installed: bool,
) -> Result<super::RuntimeRecord> {
    let existing = super::get_runtime(paths, &remote.runtime_ref)?;
    let force = force
        || existing
            .as_ref()
            .is_some_and(|record| record.origin != remote.origin);
    let runtime_dir = if !force
        && existing
            .as_ref()
            .is_some_and(|record| record.runtime_commit == remote.runtime_commit)
    {
        paths.absolute_data_path(&existing.as_ref().unwrap().runtime_dir)
    } else {
        generation_checkout_dir(
            &paths
                .runtimes()
                .join(runtime_checkout_dir(&remote.runtime_ref)),
            &remote.runtime_commit,
            force,
        )
    };
    let configured = crate::remotes::get_remote(paths, &remote.origin)?;
    let (_, summary_path, _) = load_arch_summary(paths, &configured)?;
    let summary =
        fs::read(&summary_path).with_context(|| format!("read {}", summary_path.display()))?;
    let full_ref = format!("runtime/{}", remote.runtime_ref);
    let storage = Storage::open(paths)?;
    storage.deploy(
        &summary,
        &[Deployment {
            remote: &remote.origin,
            kind: "runtime",
            ref_name: &full_ref,
            checksum: &remote.runtime_commit,
            destination: &runtime_dir,
            force,
        }],
    )?;
    let installed_size = storage.installed_size(&remote.runtime_commit)?;
    drop(storage);
    let record = super::RuntimeRecord {
        origin: remote.origin.clone(),
        runtime_ref: remote.runtime_ref.clone(),
        runtime_commit: remote.runtime_commit.clone(),
        explicitly_installed: explicitly_installed
            || existing
                .as_ref()
                .is_some_and(|record| record.explicitly_installed),
        installed_size,
        runtime_dir: paths.relative_data_path(&runtime_dir)?,
    };
    super::write_runtime(paths, &record)?;
    super::reconcile_runtime_bindings(paths)?;
    if let Some(old_origin) = existing
        .as_ref()
        .map(|record| record.origin.as_str())
        .filter(|origin| *origin != remote.origin)
    {
        remove_remote_refs(paths, old_origin, &[&full_ref])?;
    }
    Ok(record)
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

#[cfg(test)]
#[path = "tests/application_installation.rs"]
mod tests;
