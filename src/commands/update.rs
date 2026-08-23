use super::value_after_equals;
use crate::cli::transaction::{
    present_and_confirm, TransactionEntry, TransactionOperation, TransactionOptions,
};
use crate::{desktop, paths::Installation, remote, runtime, state};
use anyhow::{bail, Context, Result};
use std::collections::BTreeSet;
use std::path::Path;

pub(crate) fn cmd_update(paths: &Installation, args: Vec<String>) -> Result<()> {
    let options = parse_update_args(args)?;
    let installed = state::list_apps(paths)?;
    if installed.is_empty() {
        if let Some(app_id) = options.app_ids.first() {
            bail!("{app_id} is not installed");
        }
        println!("No installed apps");
        return Ok(());
    }
    let metadata = remote::load_remote_metadata(paths)?;
    let targets = update_targets(installed, options.app_ids, &metadata)?;
    let mut resolved = Vec::new();
    for target in targets {
        let remote = if let Some(commit) = options.commit.as_deref() {
            metadata.resolve_app_commit(paths, &target.record.app_ref, commit)?
        } else {
            match target.remote {
                Some(remote) => remote,
                None => metadata
                    .resolve_exact_ref(&target.record.app_ref)
                    .or_else(|_| metadata.resolve_app(&target.record.app_id, true))?,
            }
        };
        resolved.push((target.record, remote));
    }
    update_resolved(paths, resolved, options.transaction)
}

#[derive(Debug)]
struct ResolvedUpdate {
    record: state::AppRecord,
    remote: remote::RemoteApp,
    status: UpdateStatus,
}

pub(super) fn update_resolved(
    paths: &Installation,
    resolved: Vec<(state::AppRecord, remote::RemoteApp)>,
    options: TransactionOptions,
) -> Result<()> {
    let mut plans = Vec::new();
    for (record, remote) in resolved {
        let status = update_status(paths, &record, &remote)?;
        if !status.app_changed && !status.runtime_changed {
            if !options.noninteractive {
                println!("{} is up to date", record.app_id);
            }
            continue;
        }
        plans.push(ResolvedUpdate {
            record,
            remote,
            status,
        });
    }
    if plans.is_empty() {
        return Ok(());
    }

    let mut entries = Vec::new();
    let mut runtime_entries = BTreeSet::new();
    for plan in &plans {
        if plan.status.app_changed {
            entries.push(TransactionEntry {
                operation: TransactionOperation::Update,
                kind: "application",
                ref_name: plan.remote.app_ref.clone(),
            });
        }
        if plan.status.runtime_changed && runtime_entries.insert(plan.remote.runtime_ref.clone()) {
            entries.push(TransactionEntry {
                operation: if state::get_runtime(paths, &plan.remote.runtime_ref)?.is_some() {
                    TransactionOperation::Update
                } else {
                    TransactionOperation::Install
                },
                kind: "runtime",
                ref_name: format!("runtime/{}", plan.remote.runtime_ref),
            });
        }
    }
    if !present_and_confirm(&entries, options)? {
        return Ok(());
    }

    let mut touched_runtimes = BTreeSet::new();
    for plan in plans {
        let record = plan.record;
        let remote = plan.remote;
        let status = plan.status;
        let force_runtime =
            status.runtime_checkout_stale && touched_runtimes.insert(remote.runtime_ref.clone());
        let installed =
            runtime::update_app(paths, &remote, status.app_checkout_stale, force_runtime)?;
        let installed_record = state::record_install(paths, &installed)?;
        state::reconcile_runtime_bindings(paths)?;
        if record.app_id != installed.app_id {
            desktop::remove_export(paths, &record.app_id)?;
            state::remove_app_record(paths, &record.app_id)?;
            state::safe_remove_dir(paths, &record.app_dir)?;
        }
        let export = desktop::export_app(paths, &installed_record)?;
        if !options.noninteractive {
            println!("Updated {}", installed.app_id);
            if record.app_id != installed.app_id {
                println!("  app id: {} -> {}", record.app_id, installed.app_id);
            }
            if status.app_changed {
                println!(
                    "  app commit: {} -> {}",
                    record.app_commit, installed.app_commit
                );
            }
            if status.runtime_changed {
                println!(
                    "  runtime commit: {} -> {}",
                    status.current_runtime_commit.as_deref().unwrap_or("<none>"),
                    installed.runtime_commit
                );
            }
            print_export_report(paths, &export);
        }

        if record.runtime_ref != installed.runtime_ref
            && !state::runtime_is_required(paths, &record.runtime_ref)?
        {
            state::remove_runtime_record(paths, &record.runtime_ref)?;
        }
        state::cleanup_retired_deployments(paths)?;
    }

    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct UpdateOptions {
    pub(super) transaction: TransactionOptions,
    pub(super) commit: Option<String>,
    pub(super) app_ids: Vec<String>,
}

pub(super) fn parse_update_args(args: Vec<String>) -> Result<UpdateOptions> {
    let mut transaction = TransactionOptions::default();
    let mut commit = None;
    let mut app_ids = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-y" | "--assumeyes" => transaction.assumeyes = true,
            "--noninteractive" => transaction.noninteractive = true,
            "--commit" => {
                if commit.is_some() {
                    bail!("--commit may only be specified once");
                }
                commit = Some(args.next().context("missing value for --commit")?);
            }
            _ if arg.starts_with("--commit=") => {
                if commit.is_some() {
                    bail!("--commit may only be specified once");
                }
                commit = Some(value_after_equals(&arg).to_string());
            }
            _ if arg.starts_with('-') => bail!("unknown update option: {arg}"),
            _ => app_ids.push(arg),
        }
    }
    if commit.is_some() && app_ids.len() != 1 {
        bail!("usage: flatpak update --commit=COMMIT <app-id>");
    }
    Ok(UpdateOptions {
        transaction,
        commit,
        app_ids,
    })
}

#[derive(Debug)]
pub(super) struct UpdateTarget {
    pub(super) record: state::AppRecord,
    pub(super) remote: Option<remote::RemoteApp>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct UpdateStatus {
    pub(super) app_changed: bool,
    pub(super) app_checkout_stale: bool,
    pub(super) runtime_changed: bool,
    pub(super) runtime_checkout_stale: bool,
    pub(super) current_runtime_commit: Option<String>,
}

pub(super) fn update_targets(
    installed: Vec<state::AppRecord>,
    args: Vec<String>,
    metadata: &remote::RemoteMetadata,
) -> Result<Vec<UpdateTarget>> {
    if args.is_empty() {
        return Ok(installed
            .into_iter()
            .map(|record| UpdateTarget {
                record,
                remote: None,
            })
            .collect());
    }

    let mut targets = Vec::new();
    let mut seen = BTreeSet::new();
    for requested in args {
        let (record, remote) = if let Some(record) =
            installed.iter().find(|record| record.app_id == requested)
        {
            (record.clone(), None)
        } else {
            let remote = metadata
                .resolve_app(&requested, true)
                .with_context(|| format!("resolve {requested} in Flathub"))?;
            let record = installed
                    .iter()
                    .find(|record| record.app_id == remote.app_id)
                    .cloned()
                    .with_context(|| {
                        if remote.app_id == requested {
                            format!("{requested} is not installed")
                        } else {
                            format!(
                                "{requested} is not installed; current Flathub app id is {}, which is also not installed",
                                remote.app_id
                            )
                        }
                    })?;
            (record, Some(remote))
        };

        if seen.insert(record.app_id.clone()) {
            targets.push(UpdateTarget { record, remote });
        }
    }

    Ok(targets)
}

pub(super) fn update_status(
    paths: &Installation,
    record: &state::AppRecord,
    remote: &remote::RemoteApp,
) -> Result<UpdateStatus> {
    let app_dir = state::absolute(paths, &record.app_dir);
    let app_checkout_present = checkout_present(&app_dir);
    let app_state_changed = record.app_id != remote.app_id
        || record.app_ref != remote.app_ref
        || record.app_commit != remote.app_commit
        || record.arch != remote.arch
        || record.branch != remote.branch
        || record.command != remote.command;
    let app_checkout_stale = !app_checkout_present
        || record.app_id != remote.app_id
        || record.app_ref != remote.app_ref
        || record.app_commit != remote.app_commit;

    let runtime_record = state::get_runtime(paths, &remote.runtime_ref)?;
    let runtime_dir = runtime_record
        .as_ref()
        .map(|runtime| state::absolute(paths, &runtime.runtime_dir))
        .unwrap_or_else(|| {
            paths
                .runtimes()
                .join(runtime::runtime_checkout_dir(&remote.runtime_ref))
        });
    let available_runtime_commit = runtime_record
        .map(|runtime| runtime.runtime_commit)
        .or_else(|| {
            if record.runtime_ref == remote.runtime_ref {
                Some(record.runtime_commit.clone())
            } else {
                None
            }
        });
    let runtime_checkout_stale = !checkout_present(&runtime_dir)
        || available_runtime_commit.as_deref() != Some(remote.runtime_commit.as_str());
    let runtime_state_changed =
        record.runtime_ref != remote.runtime_ref || record.runtime_commit != remote.runtime_commit;

    Ok(UpdateStatus {
        app_changed: app_state_changed || app_checkout_stale,
        app_checkout_stale,
        runtime_changed: runtime_state_changed || runtime_checkout_stale,
        runtime_checkout_stale,
        current_runtime_commit: Some(record.runtime_commit.clone()),
    })
}

pub(super) fn checkout_present(dir: &Path) -> bool {
    dir.join("metadata").is_file() && dir.join("files").is_dir()
}

fn print_export_report(paths: &Installation, export: &desktop::ExportReport) {
    println!("  exported files: {}", export.files);
    println!("  desktop entries: {}", export.desktop_entries);
    println!(
        "  export data dir: {}",
        desktop::export_data_dir(paths).display()
    );
    if !export.skipped.is_empty() {
        let skipped = export
            .skipped
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        println!("  skipped host-incompatible exports: {skipped}");
    }
}

#[cfg(test)]
#[path = "tests/update.rs"]
mod tests;
