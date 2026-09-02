use super::confirmation::{
    present_and_confirm, TransactionEntry, TransactionOperation, TransactionOptions,
};
use crate::flatpak_ref::{set_kind_filter, FlatpakRef, PartialRef, RefKind};
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
    pub(super) no_related: bool,
    pub(super) kind: Option<RefKind>,
    pub(super) reference: Option<String>,
}

pub(super) fn parse_uninstall_args(args: Vec<String>) -> Result<UninstallOptions> {
    let mut transaction = TransactionOptions::default();
    let mut unused = false;
    let mut delete_data = false;
    let mut no_related = false;
    let mut kind = None;
    let mut operands = Vec::new();
    for arg in args {
        match arg.as_str() {
            "-y" | "--assumeyes" => transaction.assumeyes = true,
            "--noninteractive" => transaction.noninteractive = true,
            "--unused" => unused = true,
            "--delete-data" => delete_data = true,
            "--no-related" => no_related = true,
            "--app" => set_kind_filter(&mut kind, RefKind::App)?,
            "--runtime" => set_kind_filter(&mut kind, RefKind::Runtime)?,
            _ if arg.starts_with('-') => bail!("unknown uninstall option: {arg}"),
            _ => operands.push(arg),
        }
    }
    if operands.len() > 1
        || (unused && !operands.is_empty())
        || (!unused && operands.is_empty())
        || (unused && delete_data)
    {
        bail!("usage: flatpak uninstall [OPTION] [--unused | <ref>]");
    }
    Ok(UninstallOptions {
        transaction,
        unused,
        delete_data,
        no_related,
        kind,
        reference: operands.pop(),
    })
}

pub(crate) fn cmd_uninstall(paths: &Installation, args: Vec<String>) -> Result<()> {
    let options = parse_uninstall_args(args)?;
    if options.unused {
        return uninstall_unused(paths, options.transaction);
    }
    let app_id = options.reference.as_deref().context("missing app id")?;
    let record = match resolve_installed_target(paths, app_id, options.kind)? {
        InstalledTarget::App(record) => record,
        InstalledTarget::Runtime(record) => {
            return uninstall_runtime(paths, record, options.transaction);
        }
    };
    let app_id = &record.app_id;
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
    let autodelete_refs = related_refs_for_uninstall(paths, &record, options.no_related)?;
    desktop_integration::remove_export(paths, app_id)?;
    if state::remove_app_record(paths, app_id)?.is_none() {
        println!("{app_id} is not installed");
        return Ok(());
    }

    state::safe_remove_dir(paths, &record.app_dir)?;
    state::safe_remove_dir(paths, &paths.chroots().join(app_id))?;

    runtime::remove_remote_refs(paths, &record.origin, &[&record.app_ref])?;
    state::cleanup_retired_deployments(paths)?;
    if !autodelete_refs.is_empty() {
        let autodelete_plan = plan_unused_deployment_checkouts(paths)?
            .into_iter()
            .filter(|item| autodelete_refs.contains(&item.ref_name))
            .collect();
        let removed = apply_unused_deployment_plan(paths, autodelete_plan)?;
        for item in removed {
            for origin in item.origins {
                runtime::remove_remote_refs(paths, &origin, &[&item.ref_name])?;
            }
        }
    }
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

fn related_refs_for_uninstall(
    paths: &Installation,
    record: &state::AppRecord,
    no_related: bool,
) -> Result<BTreeSet<String>> {
    if no_related {
        return Ok(BTreeSet::new());
    }
    let installed_runtime_refs = state::list_runtimes(paths)?
        .into_iter()
        .map(|runtime| format!("runtime/{}", runtime.runtime_ref))
        .collect::<BTreeSet<_>>();
    runtime::autodelete_extension_refs(
        &state::absolute(paths, &record.app_dir),
        &record.app_ref,
        &record.runtime_ref,
        &state::absolute(paths, &record.runtime_dir),
        &installed_runtime_refs,
    )
}

#[derive(Debug)]
enum InstalledTarget {
    App(state::AppRecord),
    Runtime(state::RuntimeRecord),
}

fn resolve_installed_target(
    paths: &Installation,
    requested: &str,
    kind: Option<RefKind>,
) -> Result<InstalledTarget> {
    let partial = PartialRef::parse(requested)?;
    let kind = partial.effective_kind(kind)?;
    let mut matches = Vec::new();
    if kind != Some(RefKind::Runtime) {
        for app in state::list_apps(paths)? {
            if partial.matches(&FlatpakRef::parse(&app.app_ref)?) {
                matches.push(InstalledTarget::App(app));
            }
        }
    }
    if kind != Some(RefKind::App) {
        for runtime in state::list_runtimes(paths)? {
            let candidate = FlatpakRef::parse(&format!("runtime/{}", runtime.runtime_ref))?;
            if partial.matches(&candidate) {
                matches.push(InstalledTarget::Runtime(runtime));
            }
        }
    }
    match matches.len() {
        0 => bail!("{requested} is not installed"),
        1 => Ok(matches.remove(0)),
        _ => bail!(
            "{requested} matches multiple installed refs; specify kind, architecture, and branch"
        ),
    }
}

fn uninstall_runtime(
    paths: &Installation,
    record: state::RuntimeRecord,
    options: TransactionOptions,
) -> Result<()> {
    let full_ref = format!("runtime/{}", record.runtime_ref);
    let users = state::list_apps(paths)?
        .into_iter()
        .filter(|app| app.runtime_ref == record.runtime_ref)
        .map(|app| app.app_id)
        .collect::<Vec<_>>();
    if !users.is_empty() {
        bail!(
            "cannot uninstall {full_ref}: required by installed applications: {}",
            users.join(", ")
        );
    }
    let entries = [TransactionEntry {
        operation: TransactionOperation::Uninstall,
        kind: "runtime",
        ref_name: full_ref.clone(),
    }];
    if !present_and_confirm(&entries, options)? {
        return Ok(());
    }
    state::remove_runtime_record(paths, &record.runtime_ref)?;
    state::safe_remove_dir(paths, &record.runtime_dir)?;
    runtime::remove_remote_refs(paths, &record.origin, &[&full_ref])?;
    state::cleanup_retired_deployments(paths)?;
    if !options.noninteractive {
        println!("Uninstalled {full_ref}");
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
    for item in &removed {
        for origin in &item.origins {
            runtime::remove_remote_refs(paths, origin, &[&item.ref_name])?;
        }
    }
    if !options.noninteractive {
        for item in removed {
            println!("Uninstalled {}", item.ref_name);
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
    pub(super) origins: BTreeSet<String>,
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

    let installed_extension_refs = state::list_runtimes(paths)?
        .into_iter()
        .map(|runtime| format!("runtime/{}", runtime.runtime_ref))
        .collect::<BTreeSet<_>>();

    let mut required_extensions = BTreeSet::new();
    for app in &deployments {
        let runtime_dir = state::get_runtime(paths, &app.runtime_ref)?
            .map(|runtime| state::absolute(paths, &runtime.runtime_dir))
            .unwrap_or_else(|| state::absolute(paths, &app.runtime_dir));
        required_extensions.extend(runtime::required_extension_refs(
            &state::absolute(paths, &app.app_dir),
            &app.app_ref,
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

    runtime_roots.extend(
        required_extensions
            .iter()
            .filter_map(|ref_name| ref_name.strip_prefix("runtime/").map(ToOwned::to_owned)),
    );

    let mut runtime_candidates = BTreeMap::<String, (BTreeSet<PathBuf>, BTreeSet<String>)>::new();
    for runtime in state::list_runtimes(paths)? {
        if runtime.explicitly_installed {
            runtime_roots.insert(runtime.runtime_ref.clone());
        }
        let candidate = runtime_candidates.entry(runtime.runtime_ref).or_default();
        candidate
            .0
            .insert(state::absolute(paths, &runtime.runtime_dir));
        candidate.1.insert(runtime.origin);
    }
    for runtime in state::list_runtime_deployments(paths)? {
        let candidate = runtime_candidates.entry(runtime.runtime_ref).or_default();
        candidate
            .0
            .insert(state::absolute(paths, &runtime.runtime_dir));
        candidate.1.insert(runtime.origin);
    }

    let mut plan = Vec::new();
    for (runtime_ref, (deployment_paths, origins)) in runtime_candidates {
        if runtime_roots.contains(&runtime_ref) {
            continue;
        }
        plan.push(UnusedRemoval {
            ref_name: format!("runtime/{runtime_ref}"),
            kind: "runtime",
            deployment_paths,
            runtime_ref: Some(runtime_ref),
            origins,
        });
    }

    Ok(plan)
}

#[derive(Debug)]
pub(super) struct RemovedRef {
    pub(super) origins: BTreeSet<String>,
    pub(super) ref_name: String,
}

pub(super) fn apply_unused_deployment_plan(
    paths: &Installation,
    plan: Vec<UnusedRemoval>,
) -> Result<Vec<RemovedRef>> {
    let mut removed = Vec::new();
    for item in plan {
        for deployment_path in item.deployment_paths {
            state::safe_remove_dir(paths, &deployment_path)?;
        }
        if let Some(runtime_ref) = item.runtime_ref {
            state::remove_runtime_record(paths, &runtime_ref)?;
        }
        removed.push(RemovedRef {
            origins: item.origins,
            ref_name: item.ref_name,
        });
    }
    state::cleanup_retired_deployments(paths)?;
    Ok(removed)
}

#[cfg(test)]
pub(super) fn remove_unused_deployment_checkouts(paths: &Installation) -> Result<Vec<String>> {
    let plan = plan_unused_deployment_checkouts(paths)?;
    Ok(apply_unused_deployment_plan(paths, plan)?
        .into_iter()
        .map(|item| item.ref_name)
        .collect())
}

#[cfg(test)]
#[path = "tests/uninstall.rs"]
mod tests;
