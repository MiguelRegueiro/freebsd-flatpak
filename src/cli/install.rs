use super::confirmation::{
    present_and_confirm, TransactionEntry, TransactionOperation, TransactionOptions,
};
use super::update::update_resolved;
use crate::diagnostics::Diagnostics;
use crate::flatpak_ref::{set_kind_filter, RefKind};
use crate::installation as state;
use crate::installation::{self as runtime, installation_paths::Installation};
use crate::{desktop_integration, remotes};
use anyhow::{bail, Context, Result};
use std::time::Instant;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct InstallOptions {
    pub(super) transaction: TransactionOptions,
    pub(super) or_update: bool,
    pub(super) no_related: bool,
    pub(super) kind: Option<RefKind>,
    pub(super) ref_name: String,
    pub(super) remote: Option<String>,
}

pub(super) fn parse_install_args(args: Vec<String>) -> Result<InstallOptions> {
    let mut transaction = TransactionOptions::default();
    let mut or_update = false;
    let mut no_related = false;
    let mut kind = None;
    let mut operands = Vec::new();
    for arg in args {
        match arg.as_str() {
            "-y" | "--assumeyes" => transaction.assumeyes = true,
            "--noninteractive" => transaction.noninteractive = true,
            "--or-update" => or_update = true,
            "--no-related" => no_related = true,
            "--app" => set_kind_filter(&mut kind, RefKind::App)?,
            "--runtime" => set_kind_filter(&mut kind, RefKind::Runtime)?,
            _ if arg.starts_with('-') => bail!("unknown install option: {arg}"),
            _ => operands.push(arg),
        }
    }
    if !(1..=2).contains(&operands.len()) {
        bail!("usage: flatpak install [OPTION] [REMOTE] <ref>");
    }
    let ref_name = operands.pop().unwrap();
    let remote = operands.pop();
    Ok(InstallOptions {
        transaction,
        or_update,
        no_related,
        kind,
        ref_name,
        remote,
    })
}

pub(crate) fn cmd_install(
    paths: &Installation,
    args: Vec<String>,
    diagnostics: &Diagnostics,
) -> Result<()> {
    let options = parse_install_args(args)?;
    let total_started = Instant::now();
    if !options.transaction.noninteractive {
        println!("==> Resolving {}", options.ref_name);
    }
    let resolution_started = Instant::now();
    let resolved = remotes::resolve_remote_ref(
        paths,
        options.remote.as_deref(),
        &options.ref_name,
        options.kind,
    )?;
    let remote = match resolved {
        remotes::ResolvedRemoteRef::App(remote) => remote,
        remotes::ResolvedRemoteRef::Runtime(remote) => {
            return install_runtime(paths, &options, remote);
        }
    };
    let resolution = resolution_started.elapsed();
    if let Ok(record) =
        state::get_app(paths, &options.ref_name).or_else(|_| state::get_app(paths, &remote.app_id))
    {
        if !options.or_update {
            if !options.no_related {
                runtime::reconcile_extensions(paths, std::slice::from_ref(&record), false)?;
            }
            println!("{} is already installed", remote.app_id);
            return Ok(());
        }
        return update_resolved(
            paths,
            vec![(record, remote)],
            options.transaction,
            options.no_related,
            diagnostics,
            Vec::new(),
        );
    }

    let runtime_record = state::get_runtime(paths, &remote.runtime_ref)?;
    let runtime_dir = runtime_record
        .as_ref()
        .map(|record| state::absolute(paths, &record.runtime_dir))
        .unwrap_or_else(|| {
            paths
                .runtimes()
                .join(runtime::runtime_checkout_dir(&remote.runtime_ref))
        });
    let runtime_changed = runtime_record.as_ref().is_none_or(|record| {
        record.origin != remote.runtime_origin
            || record.runtime_commit != remote.runtime_commit
            || !super::update::checkout_present(&runtime_dir)
    });
    let mut entries = vec![TransactionEntry {
        operation: TransactionOperation::Install,
        kind: "application",
        ref_name: remote.app_ref.clone(),
    }];
    if runtime_changed {
        entries.push(TransactionEntry {
            operation: if runtime_record.is_some() {
                TransactionOperation::Update
            } else {
                TransactionOperation::Install
            },
            kind: "runtime",
            ref_name: format!("runtime/{}", remote.runtime_ref),
        });
    }
    if !present_and_confirm(&entries, options.transaction)? {
        return Ok(());
    }

    let mut installed = runtime::update_app(paths, &remote, true, runtime_changed)?;
    installed.timings.resolution = resolution;
    let record = state::record_install(paths, &installed)?;
    let extension_timings = if options.no_related {
        Default::default()
    } else {
        match runtime::reconcile_extensions(paths, std::slice::from_ref(&record), false) {
            Ok(timings) => timings,
            Err(error) => {
                let _ = state::remove_app_record(paths, &record.app_id);
                return Err(error).context("reconcile required extensions");
            }
        }
    };
    installed.timings.pull += extension_timings.pull;
    installed.timings.checkout += extension_timings.checkout;
    if !options.transaction.noninteractive {
        println!("\n==> Publishing desktop integration");
    }
    let export_started = Instant::now();
    let export = match desktop_integration::export_app(paths, &record) {
        Ok(export) => export,
        Err(error) => {
            let _ = desktop_integration::remove_export(paths, &record.app_id);
            let _ = state::remove_app_record(paths, &record.app_id);
            return Err(error).context("publish desktop integration");
        }
    };
    let export_elapsed = export_started.elapsed();
    if !options.transaction.noninteractive {
        println!("\n==> Installed {}", installed.app_id);
        println!("    Runtime: {}", installed.runtime_ref);
        println!("    Launch: flatpak run {}", installed.app_id);
    }
    if !options.transaction.noninteractive {
        if export.desktop_entries > 0 {
            println!("    Desktop entries: {}", export.desktop_entries);
        }
        if !export.skipped.is_empty() {
            let skipped = export
                .skipped
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            println!("    Skipped host-incompatible exports: {skipped}");
        }
        if !export.conflicts.is_empty() {
            let conflicts = export
                .conflicts
                .iter()
                .map(|path| paths.data_home().join(path).display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            println!("    Preserved conflicting user exports: {conflicts}");
        }
    }
    if std::env::var_os("FREEBSD_FLATPAK_BENCHMARK").is_some() {
        println!("\nBenchmark timings:");
        println!(
            "  resolution: {:.3}s",
            installed.timings.resolution.as_secs_f64()
        );
        println!(
            "  libostree pull: {:.3}s",
            installed.timings.pull.as_secs_f64()
        );
        println!(
            "  checkout: {:.3}s",
            installed.timings.checkout.as_secs_f64()
        );
        println!("  desktop export: {:.3}s", export_elapsed.as_secs_f64());
        println!("  total: {:.3}s", total_started.elapsed().as_secs_f64());
    }
    Ok(())
}

fn install_runtime(
    paths: &Installation,
    options: &InstallOptions,
    mut remote: remotes::RemoteRuntime,
) -> Result<()> {
    let existing = state::get_runtime(paths, &remote.runtime_ref)?;
    if options.or_update {
        if let Some(record) = existing.as_ref() {
            if record.origin != remote.origin {
                remote = remotes::load_remote_metadata(paths, &record.origin)?
                    .resolve_exact_runtime(&remote.full_ref())?;
            }
        }
    }
    let full_ref = remote.full_ref();
    if let Some(mut record) = existing.clone() {
        let current = record.runtime_commit == remote.runtime_commit
            && super::update::checkout_present(&state::absolute(paths, &record.runtime_dir));
        if !options.or_update || current {
            if !record.explicitly_installed {
                record.explicitly_installed = true;
                state::write_runtime(paths, &record)?;
            }
            println!("{full_ref} is already installed");
            return Ok(());
        }
    }
    let changed = existing
        .as_ref()
        .is_none_or(|record| record.runtime_commit != remote.runtime_commit)
        || existing.as_ref().is_some_and(|record| {
            !super::update::checkout_present(&state::absolute(paths, &record.runtime_dir))
        });
    let entries = [TransactionEntry {
        operation: if existing.is_some() {
            TransactionOperation::Update
        } else {
            TransactionOperation::Install
        },
        kind: "runtime",
        ref_name: full_ref.clone(),
    }];
    if !present_and_confirm(&entries, options.transaction)? {
        return Ok(());
    }
    state::update_runtime(paths, &remote, changed, true)?;
    state::cleanup_retired_deployments(paths)?;
    if !options.transaction.noninteractive {
        println!("\n==> Installed {full_ref}");
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/install.rs"]
mod tests;
