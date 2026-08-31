use super::confirmation::{
    present_and_confirm, TransactionEntry, TransactionOperation, TransactionOptions,
};
use crate::installation as state;
use crate::installation::{self as runtime, installation_paths::Installation};
use crate::{desktop_integration, sandbox};
use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct UninstallOptions {
    pub(super) transaction: TransactionOptions,
    pub(super) unused: bool,
    pub(super) delete_data: bool,
    pub(super) app_id: Option<String>,
}

pub(super) fn parse_uninstall_args(args: Vec<String>) -> Result<UninstallOptions> {
    let mut transaction = TransactionOptions::default();
    let mut unused = false;
    let mut delete_data = false;
    let mut operands = Vec::new();
    for arg in args {
        match arg.as_str() {
            "-y" | "--assumeyes" => transaction.assumeyes = true,
            "--noninteractive" => transaction.noninteractive = true,
            "--unused" => unused = true,
            "--delete-data" => delete_data = true,
            _ if arg.starts_with('-') => bail!("unknown uninstall option: {arg}"),
            _ => operands.push(arg),
        }
    }
    if operands.len() > 1
        || (unused && !operands.is_empty())
        || (!unused && operands.is_empty())
        || (unused && delete_data)
    {
        bail!("usage: flatpak uninstall [OPTION] [--unused | <app-id>]");
    }
    Ok(UninstallOptions {
        transaction,
        unused,
        delete_data,
        app_id: operands.pop(),
    })
}

pub(crate) fn cmd_uninstall(paths: &Installation, args: Vec<String>) -> Result<()> {
    let options = parse_uninstall_args(args)?;
    if options.unused {
        return uninstall_unused(paths, options.transaction);
    }
    let app_id = options.app_id.as_deref().context("missing app id")?;
    let record = match state::get_app(paths, app_id) {
        Ok(record) => record,
        Err(_) => {
            println!("{app_id} is not installed");
            return Ok(());
        }
    };
    if sandbox::app_has_mounts(paths, app_id)? {
        bail!("{app_id} still has active sandbox mounts; stop it before uninstalling");
    }
    let entries = [TransactionEntry {
        operation: TransactionOperation::Uninstall,
        kind: "application",
        ref_name: record.app_ref.clone(),
    }];
    if !present_and_confirm(&entries, options.transaction)? {
        return Ok(());
    }
    desktop_integration::remove_export(paths, app_id)?;
    if state::remove_app_record(paths, app_id)?.is_none() {
        println!("{app_id} is not installed");
        return Ok(());
    }

    state::safe_remove_dir(paths, &record.app_dir)?;
    state::safe_remove_dir(paths, &paths.chroots().join(app_id))?;

    runtime::remove_repo_refs(paths, &[&record.app_ref])?;
    state::cleanup_retired_deployments(paths)?;
    if options.delete_data {
        remove_app_data(paths, app_id)?;
    }
    if !options.transaction.noninteractive {
        println!("Uninstalled {app_id}");
        println!(
            "  kept runtime {} (remove with --unused)",
            record.runtime_ref
        );
        if options.delete_data {
            println!("  deleted app data");
        }
    }
    Ok(())
}

pub(super) fn remove_app_data(paths: &Installation, app_id: &str) -> Result<()> {
    let path = paths.app_data(app_id)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("inspect {}", path.display())),
    };
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(&path).with_context(|| format!("delete app data {}", path.display()))
    } else {
        fs::remove_file(&path).with_context(|| format!("delete app data {}", path.display()))
    }
}

fn uninstall_unused(paths: &Installation, options: TransactionOptions) -> Result<()> {
    let plan = plan_unused_deployment_checkouts(paths)?;
    if plan.is_empty() {
        println!("Nothing unused to uninstall");
        return Ok(());
    }
    let entries = plan
        .iter()
        .map(|item| TransactionEntry {
            operation: TransactionOperation::Uninstall,
            kind: item.kind,
            ref_name: item.ref_name.clone(),
        })
        .collect::<Vec<_>>();
    if !present_and_confirm(&entries, options)? {
        return Ok(());
    }
    let removed = apply_unused_deployment_plan(paths, plan)?;
    let refs = removed.iter().map(String::as_str).collect::<Vec<_>>();
    runtime::remove_repo_refs(paths, &refs)?;
    if !options.noninteractive {
        for ref_name in removed {
            println!("Uninstalled {ref_name}");
        }
    }
    Ok(())
}

#[derive(Debug)]
pub(super) struct UnusedRemoval {
    pub(super) ref_name: String,
    pub(super) kind: &'static str,
    pub(super) deployment_paths: BTreeSet<PathBuf>,
    pub(super) runtime_ref: Option<String>,
}

pub(super) fn plan_unused_deployment_checkouts(paths: &Installation) -> Result<Vec<UnusedRemoval>> {
    let active_gtk_theme = crate::host_resources::cursor_themes::active_gtk_theme();
    plan_unused_deployment_checkouts_with_gtk_theme(paths, active_gtk_theme.as_deref())
}

fn plan_unused_deployment_checkouts_with_gtk_theme(
    paths: &Installation,
    active_gtk_theme: Option<&str>,
) -> Result<Vec<UnusedRemoval>> {
    let apps = state::list_apps(paths)?;
    let runs = state::read_run_records(paths)?;
    let mut runtime_roots = apps
        .iter()
        .map(|app| app.runtime_ref.clone())
        .collect::<BTreeSet<_>>();
    let mut deployments = apps;
    for run in &runs {
        if let Ok(app) = state::app_from_run_record(paths, run) {
            runtime_roots.insert(app.runtime_ref.clone());
            deployments.push(app);
        }
    }

    let mut installed_extensions = Vec::new();
    if paths.extensions().is_dir() {
        for entry in fs::read_dir(paths.extensions())? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            if let Some(ref_name) = state::checkout_ref(&entry.path())? {
                installed_extensions.push((entry.path(), ref_name));
            }
        }
    }
    let installed_extension_refs = installed_extensions
        .iter()
        .map(|(_, ref_name)| ref_name.clone())
        .collect::<BTreeSet<_>>();

    let mut required_extensions = BTreeSet::new();
    for app in &deployments {
        let runtime_dir = state::get_runtime(paths, &app.runtime_ref)?
            .map(|runtime| state::absolute(paths, &runtime.runtime_dir))
            .unwrap_or_else(|| state::absolute(paths, &app.runtime_dir));
        required_extensions.extend(runtime::required_extension_refs(
            &state::absolute(paths, &app.app_dir),
            &app.runtime_ref,
            &runtime_dir,
            &installed_extension_refs,
            active_gtk_theme,
        )?);
    }
    for run in &runs {
        if let Some(refs) = run.get("extension_refs") {
            required_extensions.extend(
                refs.split(';')
                    .map(str::trim)
                    .filter(|ref_name| !ref_name.is_empty())
                    .map(ToOwned::to_owned),
            );
        }
    }

    let mut runtime_candidates = BTreeMap::<String, BTreeSet<PathBuf>>::new();
    for runtime in state::list_runtimes(paths)? {
        runtime_candidates
            .entry(runtime.runtime_ref)
            .or_default()
            .insert(state::absolute(paths, &runtime.runtime_dir));
    }
    for runtime in state::list_runtime_deployments(paths)? {
        runtime_candidates
            .entry(runtime.runtime_ref)
            .or_default()
            .insert(state::absolute(paths, &runtime.runtime_dir));
    }

    let mut plan = Vec::new();
    for (runtime_ref, deployment_paths) in runtime_candidates {
        if runtime_roots.contains(&runtime_ref) {
            continue;
        }
        plan.push(UnusedRemoval {
            ref_name: format!("runtime/{runtime_ref}"),
            kind: "runtime",
            deployment_paths,
            runtime_ref: Some(runtime_ref),
        });
    }

    for (path, ref_name) in installed_extensions {
        if required_extensions.contains(&ref_name) {
            continue;
        }
        plan.push(UnusedRemoval {
            ref_name,
            kind: "extension",
            deployment_paths: BTreeSet::from([path]),
            runtime_ref: None,
        });
    }

    Ok(plan)
}

pub(super) fn apply_unused_deployment_plan(
    paths: &Installation,
    plan: Vec<UnusedRemoval>,
) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    for item in plan {
        for deployment_path in item.deployment_paths {
            state::safe_remove_dir(paths, &deployment_path)?;
        }
        if let Some(runtime_ref) = item.runtime_ref {
            state::remove_runtime_record(paths, &runtime_ref)?;
        }
        removed.push(item.ref_name);
    }
    state::cleanup_retired_deployments(paths)?;
    Ok(removed)
}

#[cfg(test)]
pub(super) fn remove_unused_deployment_checkouts(paths: &Installation) -> Result<Vec<String>> {
    let plan = plan_unused_deployment_checkouts(paths)?;
    apply_unused_deployment_plan(paths, plan)
}

#[cfg(test)]
#[path = "tests/uninstall.rs"]
mod tests;
