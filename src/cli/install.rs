use super::confirmation::{
    present_and_confirm, TransactionEntry, TransactionOperation, TransactionOptions,
};
use super::update::update_resolved;
use crate::installation as state;
use crate::installation::{self as runtime, installation_paths::Installation};
use crate::{desktop_integration, remotes};
use anyhow::{bail, Context, Result};
use std::time::Instant;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct InstallOptions {
    pub(super) transaction: TransactionOptions,
    pub(super) or_update: bool,
    pub(super) app_id: String,
    pub(super) remote: Option<String>,
}

pub(super) fn parse_install_args(args: Vec<String>) -> Result<InstallOptions> {
    let mut transaction = TransactionOptions::default();
    let mut or_update = false;
    let mut operands = Vec::new();
    for arg in args {
        match arg.as_str() {
            "-y" | "--assumeyes" => transaction.assumeyes = true,
            "--noninteractive" => transaction.noninteractive = true,
            "--or-update" => or_update = true,
            _ if arg.starts_with('-') => bail!("unknown install option: {arg}"),
            _ => operands.push(arg),
        }
    }
    if !(1..=2).contains(&operands.len()) {
        bail!("usage: flatpak install [OPTION] [REMOTE] <app-id>");
    }
    let app_id = operands.pop().unwrap();
    let remote = operands.pop();
    Ok(InstallOptions {
        transaction,
        or_update,
        app_id,
        remote,
    })
}

pub(crate) fn cmd_install(paths: &Installation, args: Vec<String>) -> Result<()> {
    let options = parse_install_args(args)?;
    let total_started = Instant::now();
    if !options.transaction.noninteractive {
        println!("==> Resolving {}", options.app_id);
    }
    let resolution_started = Instant::now();
    let remote = remotes::resolve_remote_app(paths, options.remote.as_deref(), &options.app_id)?;
    let resolution = resolution_started.elapsed();
    if let Ok(record) =
        state::get_app(paths, &options.app_id).or_else(|_| state::get_app(paths, &remote.app_id))
    {
        if !options.or_update {
            println!("{} is already installed", remote.app_id);
            return Ok(());
        }
        return update_resolved(paths, vec![(record, remote)], options.transaction);
    }

    let runtime_record =
        state::get_runtime_from(paths, &remote.runtime_origin, &remote.runtime_ref)?;
    let runtime_dir = runtime_record
        .as_ref()
        .map(|record| state::absolute(paths, &record.runtime_dir))
        .unwrap_or_else(|| {
            paths
                .runtimes()
                .join(&remote.runtime_origin)
                .join(runtime::runtime_checkout_dir(&remote.runtime_ref))
        });
    let runtime_changed = runtime_record
        .as_ref()
        .map(|record| record.runtime_commit.as_str())
        != Some(remote.runtime_commit.as_str())
        || !super::update::checkout_present(&runtime_dir);
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

#[cfg(test)]
#[path = "tests/install.rs"]
mod tests;
