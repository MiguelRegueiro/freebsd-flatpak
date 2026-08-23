use super::installation_paths::Installation;
use crate::extensions::{ensure_default_gl_extension_timed, runtime_checkout_dir};
use crate::ostree::{Deployment, Storage};
use crate::remotes::{load_arch_summary, RemoteApp};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct InstalledApp {
    pub app_id: String,
    pub app_ref: String,
    pub app_commit: String,
    pub app_dir: PathBuf,
    pub arch: String,
    pub branch: String,
    pub runtime_ref: String,
    pub runtime_commit: String,
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
    let runtime_dir = generation_checkout_dir(
        &paths
            .runtimes()
            .join(runtime_checkout_dir(&remote.runtime_ref)),
        &remote.runtime_commit,
        force_runtime,
    );
    let (_, summary_path, _) = load_arch_summary(paths)?;
    let summary =
        fs::read(&summary_path).with_context(|| format!("read {}", summary_path.display()))?;
    let runtime_full_ref = format!("runtime/{}", remote.runtime_ref);
    let storage = Storage::open(paths)?;
    let mut timings = storage.deploy(
        &summary,
        &[
            Deployment {
                kind: "application",
                ref_name: &remote.app_ref,
                checksum: &remote.app_commit,
                destination: &app_dir,
                force: force_app,
            },
            Deployment {
                kind: "runtime",
                ref_name: &runtime_full_ref,
                checksum: &remote.runtime_commit,
                destination: &runtime_dir,
                force: force_runtime,
            },
        ],
    )?;
    drop(storage);
    let (_, extension_timings) =
        ensure_default_gl_extension_timed(paths, &remote.runtime_ref, &runtime_dir)?;
    timings.pull += extension_timings.pull;
    timings.checkout += extension_timings.checkout;

    Ok(InstalledApp {
        app_id: remote.app_id.clone(),
        app_ref: remote.app_ref.clone(),
        app_commit: remote.app_commit.clone(),
        app_dir,
        arch: remote.arch.clone(),
        branch: remote.branch.clone(),
        runtime_ref: remote.runtime_ref.clone(),
        runtime_commit: remote.runtime_commit.clone(),
        runtime_dir,
        command: remote.command.clone(),
        timings: InstallTimings {
            resolution: Duration::ZERO,
            pull: timings.pull,
            checkout: timings.checkout,
        },
    })
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
