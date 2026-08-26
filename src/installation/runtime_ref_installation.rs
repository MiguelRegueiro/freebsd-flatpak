use super::{extension_checkout_dir, runtime_checkout_dir, write_runtime, RuntimeRecord};
use crate::installation::installation_paths::Installation;
use crate::ostree::{Deployment, Storage};
use crate::remotes::{load_arch_summary, RemoteRuntime};
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

pub(crate) struct InstalledRuntimeRef {
    pub checkout_dir: PathBuf,
    pub installed_size: u64,
}

pub(crate) fn install_runtime_ref(
    paths: &Installation,
    remote: &RemoteRuntime,
) -> Result<InstalledRuntimeRef> {
    let checkout_dir = if remote.is_extension {
        extension_checkout_dir(paths, &remote.ref_name)?
    } else {
        paths
            .runtimes()
            .join(&remote.origin)
            .join(runtime_checkout_dir(&remote.runtime_ref))
            .join(&remote.commit)
    };
    let configured = crate::remotes::get_remote(paths, &remote.origin)?;
    let (_, summary_path, _) = load_arch_summary(paths, &configured)?;
    let summary =
        fs::read(&summary_path).with_context(|| format!("read {}", summary_path.display()))?;
    let storage = Storage::open(paths)?;
    storage.deploy(
        &summary,
        &[Deployment {
            remote: &remote.origin,
            kind: if remote.is_extension {
                "extension"
            } else {
                "runtime"
            },
            ref_name: &remote.ref_name,
            checksum: &remote.commit,
            destination: &checkout_dir,
            force: false,
        }],
    )?;
    let installed_size = storage.installed_size(&remote.commit)?;
    drop(storage);

    if !remote.is_extension {
        write_runtime(
            paths,
            &RuntimeRecord {
                origin: remote.origin.clone(),
                runtime_ref: remote.runtime_ref.clone(),
                runtime_commit: remote.commit.clone(),
                installed_size,
                runtime_dir: paths.relative_data_path(&checkout_dir)?,
            },
        )?;
    }

    Ok(InstalledRuntimeRef {
        checkout_dir,
        installed_size,
    })
}
