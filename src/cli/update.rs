use super::confirmation::{confirm_after_preview, TransactionOptions};
use super::update_output::{render, short_change, UpdateRow};
use crate::diagnostics::{Detail, Diagnostics};
use crate::flatpak_ref::{set_kind_filter, FlatpakRef, PartialRef, RefKind};
use crate::installation as state;
use crate::installation::{self as runtime, installation_paths::Installation};
use crate::{desktop_integration, remotes};
use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub(crate) fn cmd_update(
    paths: &Installation,
    args: Vec<String>,
    diagnostics: &Diagnostics,
) -> Result<()> {
    let mut options = parse_update_args(args)?;
    let had_requested_refs = !options.refs.is_empty();
    let installed = state::list_apps(paths)?;
    let (runtime_targets, runtime_requests) = select_runtime_targets(paths, &options, &installed)?;
    let has_runtime_targets = !runtime_targets.is_empty();
    if has_runtime_targets {
        update_runtime_targets(
            paths,
            runtime_targets,
            options.transaction,
            options.commit.as_deref(),
            diagnostics,
        )?;
    }
    options
        .refs
        .retain(|requested| !runtime_requests.contains(requested));
    if !should_update_apps(&options, had_requested_refs) {
        if options.kind == Some(RefKind::Runtime) && !has_runtime_targets {
            println!("No installed runtimes.");
        }
        return Ok(());
    }
    diagnostics.message(Detail::Summary, || {
        format!("update: found {} installed application(s)", installed.len())
    });
    if installed.is_empty() {
        if let Some(app_id) = options.refs.first() {
            bail!("{app_id} is not installed");
        }
        if !has_runtime_targets && runtime_requests.is_empty() {
            println!("No installed refs.");
        }
        return Ok(());
    }
    if !options.transaction.noninteractive {
        println!("Checking for updates…");
    }
    let targets = if options.refs.is_empty() {
        installed
            .into_iter()
            .map(|record| UpdateTarget {
                record,
                remote: None,
            })
            .collect()
    } else {
        let mut targets = Vec::new();
        let mut seen = BTreeSet::new();
        for requested in &options.refs {
            if let Some(record) = installed.iter().find(|record| record.app_id == *requested) {
                if seen.insert(record.app_id.clone()) {
                    targets.push(UpdateTarget {
                        record: record.clone(),
                        remote: None,
                    });
                }
                continue;
            }
            let mut matched = None;
            for origin in installed
                .iter()
                .map(|record| record.origin.as_str())
                .collect::<BTreeSet<_>>()
            {
                if let Ok(remote) = remotes::resolve_remote_app(paths, Some(origin), requested) {
                    if let Some(record) = installed
                        .iter()
                        .find(|record| record.origin == origin && record.app_id == remote.app_id)
                    {
                        matched = Some(UpdateTarget {
                            record: record.clone(),
                            remote: Some(remote),
                        });
                        break;
                    }
                }
            }
            let target = matched.with_context(|| format!("{requested} is not installed"))?;
            if seen.insert(target.record.app_id.clone()) {
                targets.push(target);
            }
        }
        targets
    };
    diagnostics.message(Detail::Summary, || {
        format!("update: considering {} application ref(s)", targets.len())
    });
    let mut resolved = Vec::new();
    let mut transaction_metadata = BTreeMap::new();
    for target in targets {
        diagnostics.message(Detail::Summary, || {
            format!(
                "update: refresh remote metadata {} for {}",
                target.record.origin, target.record.app_id
            )
        });
        if !transaction_metadata.contains_key(&target.record.origin) {
            let metadata = diagnostics
                .measure(Detail::Detailed, "update", "load remote metadata", || {
                    remotes::load_remote_metadata(paths, &target.record.origin)
                })
                .with_context(|| {
                    format!(
                        "load origin {} for {}",
                        target.record.origin, target.record.app_id
                    )
                })?;
            transaction_metadata.insert(target.record.origin.clone(), metadata);
        }
        let metadata = transaction_metadata
            .get(&target.record.origin)
            .expect("inserted transaction metadata");
        let remote = diagnostics.measure(
            Detail::Detailed,
            "update",
            "resolve application ref",
            || {
                if let Some(commit) = options.commit.as_deref() {
                    metadata.resolve_app_commit(paths, &target.record.app_ref, commit)
                } else {
                    match target.remote {
                        Some(remote) => Ok(remote),
                        None => metadata
                            .resolve_exact_ref_with_runtime(paths, &target.record.app_ref)
                            .or_else(|_| {
                                remotes::resolve_remote_app(
                                    paths,
                                    Some(&target.record.origin),
                                    &target.record.app_id,
                                )
                            }),
                    }
                }
            },
        )?;
        diagnostics.message(Detail::Detailed, || {
            format!(
                "update: resolved {} to {} (runtime runtime/{})",
                remote.app_ref, remote.app_commit, remote.runtime_ref
            )
        });
        resolved.push((target.record, remote));
    }
    update_resolved(
        paths,
        resolved,
        options.transaction,
        options.no_related,
        diagnostics,
        transaction_metadata.into_values().collect(),
    )
}
fn should_update_apps(options: &UpdateOptions, had_requested_refs: bool) -> bool {
    options.kind != Some(RefKind::Runtime) && (!had_requested_refs || !options.refs.is_empty())
}

fn select_runtime_targets(
    paths: &Installation,
    options: &UpdateOptions,
    apps: &[state::AppRecord],
) -> Result<(Vec<state::RuntimeRecord>, BTreeSet<String>)> {
    let runtimes = state::list_runtimes(paths)?;
    if options.refs.is_empty() {
        return Ok(if options.kind == Some(RefKind::App) {
            (Vec::new(), BTreeSet::new())
        } else {
            (runtimes, BTreeSet::new())
        });
    }
    let mut targets = Vec::new();
    let mut matched_requests = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for requested in &options.refs {
        let partial = PartialRef::parse(requested)?;
        let kind = partial.effective_kind(options.kind)?;
        if kind == Some(RefKind::App) {
            continue;
        }
        let mut runtime_matches = Vec::new();
        for record in &runtimes {
            let candidate = FlatpakRef::parse(&format!("runtime/{}", record.runtime_ref))?;
            if partial.matches(&candidate) {
                runtime_matches.push(record);
            }
        }
        let mut app_match = false;
        if kind.is_none() {
            for record in apps {
                if partial.matches(&FlatpakRef::parse(&record.app_ref)?) {
                    app_match = true;
                    break;
                }
            }
        }
        if app_match && !runtime_matches.is_empty() {
            bail!("{requested} matches both an application and a runtime; use --app or --runtime");
        }
        if runtime_matches.is_empty() {
            if kind == Some(RefKind::Runtime) {
                bail!("{requested} is not installed");
            }
            continue;
        }
        if options.commit.is_some() && runtime_matches.len() > 1 {
            bail!(
                "{requested} matches multiple installed runtimes; specify a full ref with --commit"
            );
        }
        for record in runtime_matches {
            if seen.insert(record.runtime_ref.clone()) {
                targets.push(record.clone());
            }
        }
        matched_requests.insert(requested.clone());
    }
    Ok((targets, matched_requests))
}

fn update_runtime_targets(
    paths: &Installation,
    targets: Vec<state::RuntimeRecord>,
    options: TransactionOptions,
    commit: Option<&str>,
    diagnostics: &Diagnostics,
) -> Result<()> {
    let mut plans = Vec::new();
    for record in targets {
        let metadata = diagnostics.measure(
            Detail::Detailed,
            "update",
            "load runtime remote metadata",
            || remotes::load_remote_metadata(paths, &record.origin),
        )?;
        let full_ref = format!("runtime/{}", record.runtime_ref);
        let remote = if let Some(commit) = commit {
            metadata.resolve_runtime_commit(paths, &full_ref, commit)?
        } else {
            metadata.resolve_exact_runtime(&full_ref)?
        };
        let stale = record.runtime_commit != remote.runtime_commit
            || !checkout_present(&state::absolute(paths, &record.runtime_dir));
        if stale {
            plans.push((record, remote));
        }
    }
    if plans.is_empty() {
        return Ok(());
    }
    let rows = plans
        .iter()
        .map(|(_, remote)| UpdateRow::runtime(&remote.runtime_ref, &remote.origin))
        .collect::<Vec<_>>();
    if options.noninteractive {
        for row in &rows {
            println!("Updating {}", row.id);
        }
    } else {
        print!("{}", render(&rows));
    }
    if !confirm_after_preview(options)? {
        return Ok(());
    }
    for (record, remote) in plans {
        runtime::update_runtime(paths, &remote, true, record.explicitly_installed)?;
    }
    state::cleanup_retired_deployments(paths)?;
    Ok(())
}

#[derive(Debug)]
struct ResolvedUpdate {
    record: state::AppRecord,
    remote: remotes::RemoteApp,
    status: UpdateStatus,
}

pub(super) fn update_resolved(
    paths: &Installation,
    resolved: Vec<(state::AppRecord, remotes::RemoteApp)>,
    options: TransactionOptions,
    no_related: bool,
    diagnostics: &Diagnostics,
    remote_metadata: Vec<remotes::RemoteMetadata>,
) -> Result<()> {
    let mut plans = Vec::new();
    let mut selected_apps = Vec::new();
    for (record, remote) in resolved {
        let status = update_status(paths, &record, &remote)?;
        diagnostics.message(Detail::Detailed, || {
            format!(
                "update: status {} app_changed={} app_checkout_stale={} runtime_changed={} runtime_checkout_stale={}",
                record.app_id,
                status.app_changed,
                status.app_checkout_stale,
                status.runtime_changed,
                status.runtime_checkout_stale
            )
        });
        if !status.app_changed && !status.runtime_changed {
            diagnostics.message(Detail::Summary, || {
                format!("update: keep {} (already current)", record.app_ref)
            });
            selected_apps.push(record);
            continue;
        }
        diagnostics.message(Detail::Summary, || {
            let mut changes = Vec::new();
            if status.app_changed {
                changes.push("application");
            }
            if status.runtime_changed {
                changes.push("runtime");
            }
            format!(
                "update: select {} ({})",
                record.app_ref,
                changes.join(" and ")
            )
        });
        plans.push(ResolvedUpdate {
            record,
            remote,
            status,
        });
    }
    if plans.is_empty() {
        let extension_timings = if no_related {
            Default::default()
        } else {
            runtime::reconcile_extensions_with_metadata(
                paths,
                &selected_apps,
                false,
                remote_metadata,
                diagnostics.enabled(Detail::Summary),
            )?
        };
        if !options.noninteractive {
            if extension_timings.checkout.is_zero() {
                println!("Nothing to update.");
            } else {
                println!("Extension update complete.");
            }
        }
        return Ok(());
    }

    let mut rows = Vec::new();
    let mut runtime_entries = BTreeSet::new();
    for plan in &plans {
        if plan.status.app_changed {
            diagnostics.message(Detail::Summary, || {
                format!(
                    "update: plan update application {}",
                    short_change(
                        &plan.record.app_ref,
                        &plan.remote.app_ref,
                        &plan.record.app_commit,
                        &plan.remote.app_commit,
                    )
                )
            });
            rows.push(UpdateRow::application(
                &plan.remote.app_id,
                &plan.remote.branch,
                &plan.remote.origin,
            ));
        }
        if plan.status.runtime_changed && runtime_entries.insert(plan.remote.runtime_ref.clone()) {
            let operation = if state::get_runtime(paths, &plan.remote.runtime_ref)?.is_some() {
                "update"
            } else {
                "install"
            };
            diagnostics.message(Detail::Summary, || {
                format!(
                    "update: plan {operation} runtime {}",
                    short_change(
                        &format!("runtime/{}", plan.record.runtime_ref),
                        &format!("runtime/{}", plan.remote.runtime_ref),
                        plan.status
                            .current_runtime_commit
                            .as_deref()
                            .unwrap_or("<none>"),
                        &plan.remote.runtime_commit,
                    )
                )
            });
            rows.push(UpdateRow::runtime(
                &plan.remote.runtime_ref,
                &plan.remote.runtime_origin,
            ));
        }
    }
    diagnostics.message(Detail::Summary, || {
        format!("update: planned {} transaction operation(s)", rows.len())
    });
    if options.noninteractive {
        for row in &rows {
            println!("Updating {}", row.id);
        }
    } else {
        print!("{}", render(&rows));
    }
    if !confirm_after_preview(options)? {
        return Ok(());
    }

    let mut touched_runtimes = BTreeSet::new();
    for plan in plans {
        let record = plan.record;
        let remote = plan.remote;
        let status = plan.status;
        let force_runtime =
            status.runtime_checkout_stale && touched_runtimes.insert(remote.runtime_ref.clone());
        diagnostics.message(Detail::Summary, || {
            format!(
                "update: deploy {} (application={}, runtime={})",
                remote.app_ref, status.app_checkout_stale, force_runtime
            )
        });
        diagnostics.message(Detail::Detailed, || {
            format!(
                "update: commits {} -> {}; runtime {} -> {}",
                record.app_commit,
                remote.app_commit,
                status.current_runtime_commit.as_deref().unwrap_or("<none>"),
                remote.runtime_commit
            )
        });
        let installed =
            diagnostics.measure(Detail::Detailed, "update", "pull and deploy", || {
                runtime::update_app(paths, &remote, status.app_checkout_stale, force_runtime)
            })?;
        let installed_record = state::record_install(paths, &installed)?;
        selected_apps.push(installed_record.clone());
        state::reconcile_runtime_bindings(paths)?;
        if record.app_id != installed.app_id {
            desktop_integration::remove_export(paths, &record.app_id)?;
            state::remove_app_record(paths, &record.app_id)?;
            state::safe_remove_dir(paths, &record.app_dir)?;
        }
        let export = desktop_integration::export_app(paths, &installed_record)?;
        diagnostics.message(Detail::Summary, || {
            format!("update: activated {}", installed.app_id)
        });
        diagnostics.message(Detail::Detailed, || export_report(paths, &export));

        if record.runtime_ref != installed.runtime_ref
            && !state::runtime_is_required(paths, &record.runtime_ref)?
        {
            if let Some(old_runtime) = state::get_runtime(paths, &record.runtime_ref)? {
                if !old_runtime.explicitly_installed {
                    state::remove_runtime_record(paths, &old_runtime.runtime_ref)?;
                    let full_ref = format!("runtime/{}", old_runtime.runtime_ref);
                    runtime::remove_remote_refs(paths, &old_runtime.origin, &[&full_ref])?;
                }
            }
        }
        state::cleanup_retired_deployments(paths)?;
    }

    if !no_related {
        diagnostics.measure(Detail::Detailed, "update", "reconcile extensions", || {
            runtime::reconcile_extensions_with_metadata(
                paths,
                &selected_apps,
                false,
                remote_metadata,
                diagnostics.enabled(Detail::Summary),
            )
        })?;
    }

    if !options.noninteractive {
        println!("Update complete.");
    }

    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct UpdateOptions {
    pub(super) transaction: TransactionOptions,
    pub(super) commit: Option<String>,
    pub(super) no_related: bool,
    pub(super) kind: Option<RefKind>,
    pub(super) refs: Vec<String>,
}

pub(super) fn parse_update_args(args: Vec<String>) -> Result<UpdateOptions> {
    let mut transaction = TransactionOptions::default();
    let mut commit = None;
    let mut no_related = false;
    let mut kind = None;
    let mut refs = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-y" | "--assumeyes" => transaction.assumeyes = true,
            "--noninteractive" => transaction.noninteractive = true,
            "--app" => set_kind_filter(&mut kind, RefKind::App)?,
            "--runtime" => set_kind_filter(&mut kind, RefKind::Runtime)?,
            "--no-related" => no_related = true,
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
            _ => refs.push(arg),
        }
    }
    if commit.is_some() && refs.len() != 1 {
        bail!("usage: flatpak update --commit=COMMIT <ref>");
    }
    Ok(UpdateOptions {
        transaction,
        commit,
        no_related,
        kind,
        refs,
    })
}

#[derive(Debug)]
pub(super) struct UpdateTarget {
    pub(super) record: state::AppRecord,
    pub(super) remote: Option<remotes::RemoteApp>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct UpdateStatus {
    pub(super) app_changed: bool,
    pub(super) app_checkout_stale: bool,
    pub(super) runtime_changed: bool,
    pub(super) runtime_checkout_stale: bool,
    pub(super) current_runtime_commit: Option<String>,
}

#[cfg(test)]
pub(super) fn update_targets(
    installed: Vec<state::AppRecord>,
    args: Vec<String>,
    metadata: &remotes::RemoteMetadata,
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
    remote: &remotes::RemoteApp,
) -> Result<UpdateStatus> {
    let app_dir = state::absolute(paths, &record.app_dir);
    let app_checkout_present = checkout_present(&app_dir);
    let app_state_changed = record.origin != remote.origin
        || record.app_id != remote.app_id
        || record.app_ref != remote.app_ref
        || record.app_commit != remote.app_commit
        || record.arch != remote.arch
        || record.branch != remote.branch
        || record.command != remote.command;
    let app_checkout_stale = !app_checkout_present
        || record.origin != remote.origin
        || record.app_id != remote.app_id
        || record.app_ref != remote.app_ref
        || record.app_commit != remote.app_commit;

    let runtime_record = state::get_runtime(paths, &remote.runtime_ref)?;
    let runtime_dir = runtime_record
        .as_ref()
        .map(|runtime| state::absolute(paths, &runtime.runtime_dir))
        .unwrap_or_else(|| {
            if record.runtime_ref == remote.runtime_ref {
                state::absolute(paths, &record.runtime_dir)
            } else {
                paths
                    .runtimes()
                    .join(runtime::runtime_checkout_dir(&remote.runtime_ref))
            }
        });
    let available_runtime_commit = runtime_record
        .as_ref()
        .map(|runtime| runtime.runtime_commit.clone())
        .or_else(|| {
            if record.runtime_ref == remote.runtime_ref {
                Some(record.runtime_commit.clone())
            } else {
                None
            }
        });
    let runtime_checkout_stale = runtime_record
        .as_ref()
        .is_some_and(|runtime| runtime.origin != remote.runtime_origin)
        || !checkout_present(&runtime_dir)
        || available_runtime_commit.as_deref() != Some(remote.runtime_commit.as_str());
    let runtime_state_changed = record.runtime_origin != remote.runtime_origin
        || record.runtime_ref != remote.runtime_ref
        || record.runtime_commit != remote.runtime_commit;

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

fn export_report(paths: &Installation, export: &desktop_integration::ExportReport) -> String {
    let mut report = format!(
        "update: exports files={} desktop_entries={} data_dir={}",
        export.files,
        export.desktop_entries,
        desktop_integration::export_data_dir(paths).display()
    );
    if !export.skipped.is_empty() {
        let skipped = export
            .skipped
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        report.push_str(&format!(" skipped={skipped}"));
    }
    if !export.conflicts.is_empty() {
        let conflicts = export
            .conflicts
            .iter()
            .map(|path| paths.data_home().join(path).display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        report.push_str(&format!(" conflicts={conflicts}"));
    }
    report
}

fn value_after_equals(arg: &str) -> &str {
    arg.split_once('=').map(|(_, value)| value).unwrap_or("")
}

#[cfg(test)]
#[path = "tests/update.rs"]
mod tests;
