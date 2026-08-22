mod audio;
mod cursor;
mod desktop;
mod filesystem;
mod fonts;
mod graphics;
mod linuxulator;
mod paths;
mod portal;
mod ps;
mod runtime;
mod sandbox;
mod state;
mod storage;
mod video;

use anyhow::{bail, Context, Result};
use paths::Installation;
use sandbox::SandboxBackend;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Instant;

const HELP: &str = r#"Usage:
  flatpak [OPTION] COMMAND

Commands:
  install       Install an application
  update        Update installed applications
  remote-info   Show information about an application in a remote
  uninstall     Uninstall an application
  list          List installed applications
  search        Search Flathub
  run           Run an application
  ps            List running applications
  permissions   Show application permissions
  repair        Verify and repair the installation
  prune         Remove unused stored data

Options:
  -h, --help    Show help
"#;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    if matches!(command.as_deref(), Some("-h" | "--help")) {
        print_help();
        return Ok(());
    }

    let paths = Installation::from_env()?;
    state::ensure_layout(&paths)?;
    sandbox::recover_stale_mounts(&paths)?;
    portal::recover_stale_portal_mounts(&paths)?;
    graphics::recover_stale_graphics_dirs(&paths)?;

    match command.as_deref() {
        Some("search") => cmd_search(&paths, args.collect()),
        Some("install") => cmd_install(&paths, args.collect()),
        Some("list") => cmd_list(&paths, args.collect()),
        Some("permissions") => cmd_permissions(&paths, args.collect()),
        Some("ps") => cmd_ps(&paths, args.collect()),
        Some("prune") => cmd_prune(&paths, args.collect()),
        Some("repair") => cmd_repair(&paths, args.collect()),
        Some("run") => cmd_run(&paths, args.collect()),
        Some("remote-info") => cmd_remote_info(&paths, args.collect()),
        Some("uninstall") => cmd_uninstall(&paths, args.collect()),
        Some("update") => cmd_update(&paths, args.collect()),
        Some("checkout") => {
            let ref_name = args.next().context("missing ref")?;
            let dest = args.next().context("missing destination")?;
            runtime::checkout_ref(&paths, &ref_name, PathBuf::from(dest))
        }
        Some("inspect") => {
            let refs: Vec<String> = args.collect();
            runtime::inspect_refs(&paths, &refs)
        }
        Some(cmd) => anyhow::bail!("unknown command: {cmd}"),
        None => {
            print_usage();
            Ok(())
        }
    }
}

fn cmd_search(paths: &Installation, args: Vec<String>) -> Result<()> {
    if args.len() != 1 {
        bail!("usage: flatpak search <query>");
    }
    let results = runtime::search_apps(paths, &args[0])?;
    if results.is_empty() {
        println!("No matches");
        return Ok(());
    }
    println!(
        "{:<42} {:<8} {:<12} Ref",
        "Application ID", "Arch", "Branch"
    );
    for result in results.into_iter().take(50) {
        println!(
            "{:<42} {:<8} {:<12} {}",
            result.app_id, result.arch, result.branch, result.app_ref
        );
    }
    Ok(())
}

fn cmd_remote_info(paths: &Installation, args: Vec<String>) -> Result<()> {
    let options = parse_remote_info_args(args)?;
    let metadata = runtime::load_remote_metadata(paths)?;
    let remote = metadata.resolve_app(&options.app_id, true)?;
    let appstream = match metadata.appstream_info(&remote.app_id) {
        Ok(info) => info,
        Err(error) => {
            eprintln!("warning: load Flathub AppStream metadata: {error:#}");
            None
        }
    };
    let styled = std::io::stdout().is_terminal();

    if options.log {
        let (_, history) = metadata.app_history(paths, &options.app_id)?;
        print!(
            "{}",
            remote_log_output(
                &remote,
                &history,
                appstream.as_ref(),
                metadata.collection_id(),
                styled,
            )
        );
    } else if let Some(commit) = options.commit {
        let current_commit = remote.app_commit.clone();
        let (remote, commit) = metadata.app_commit(paths, &remote.app_ref, &commit)?;
        let historical = commit.checksum != current_commit;
        print!(
            "{}",
            remote_info_output(
                &remote,
                Some(&commit),
                appstream.as_ref(),
                metadata.collection_id(),
                historical,
                styled,
            )
        );
    } else {
        print!(
            "{}",
            remote_info_output(
                &remote,
                None,
                appstream.as_ref(),
                metadata.collection_id(),
                false,
                styled,
            )
        );
    }
    Ok(())
}

fn remote_log_output(
    remote: &runtime::RemoteApp,
    history: &[storage::CommitInfo],
    appstream: Option<&runtime::AppstreamInfo>,
    remote_collection: Option<&str>,
    styled: bool,
) -> String {
    let current = history.first();
    let version = current
        .and_then(|commit| commit.version.as_deref())
        .or_else(|| appstream.and_then(|info| info.version.as_deref()));
    let collection = current
        .and_then(|commit| commit.collection_id.as_deref())
        .or(remote_collection);
    let mut output = remote_metadata_output(
        remote,
        appstream,
        version,
        appstream.and_then(|info| info.license.as_deref()),
        collection,
        styled,
    );
    if let Some(current) = history.first() {
        output.push('\n');
        append_commit(&mut output, current, true, styled);
    }
    if history.len() > 1 {
        append_label(&mut output, "History:", None, styled);
        for commit in &history[1..] {
            output.push('\n');
            append_commit(&mut output, commit, false, styled);
        }
    }
    output
}

fn remote_info_output(
    remote: &runtime::RemoteApp,
    commit: Option<&storage::CommitInfo>,
    appstream: Option<&runtime::AppstreamInfo>,
    remote_collection: Option<&str>,
    historical: bool,
    styled: bool,
) -> String {
    let version = commit
        .and_then(|commit| commit.version.as_deref())
        .or_else(|| {
            (!historical)
                .then(|| appstream.and_then(|info| info.version.as_deref()))
                .flatten()
        });
    let license = (!historical)
        .then(|| appstream.and_then(|info| info.license.as_deref()))
        .flatten();
    let collection = commit
        .and_then(|commit| commit.collection_id.as_deref())
        .or_else(|| (!historical).then_some(remote_collection).flatten());
    let mut output =
        remote_metadata_output(remote, appstream, version, license, collection, styled);
    output.push('\n');
    if let Some(commit) = commit {
        append_commit(&mut output, commit, true, styled);
    } else {
        append_label(&mut output, "Commit:", Some(&remote.app_commit), styled);
    }
    output
}

fn remote_metadata_output(
    remote: &runtime::RemoteApp,
    appstream: Option<&runtime::AppstreamInfo>,
    version: Option<&str>,
    license: Option<&str>,
    collection: Option<&str>,
    styled: bool,
) -> String {
    let mut output = String::from("\n");
    if let Some(name) = appstream.and_then(|info| info.name.as_deref()) {
        output.push_str(name);
        if let Some(summary) = appstream.and_then(|info| info.summary.as_deref()) {
            output.push_str(" - ");
            output.push_str(summary);
        }
        output.push_str("\n\n");
    }
    append_label(&mut output, "ID:", Some(&remote.app_id), styled);
    append_label(&mut output, "Ref:", Some(&remote.app_ref), styled);
    append_label(&mut output, "Arch:", Some(&remote.arch), styled);
    append_label(&mut output, "Branch:", Some(&remote.branch), styled);
    if let Some(version) = version {
        append_label(&mut output, "Version:", Some(version), styled);
    }
    if let Some(license) = license {
        append_label(&mut output, "License:", Some(license), styled);
    }
    if let Some(collection) = collection {
        append_label(&mut output, "Collection:", Some(collection), styled);
    }
    if let Some(size) = remote.download_size {
        append_label(
            &mut output,
            "Download Size:",
            Some(&format_remote_size(size)),
            styled,
        );
    }
    if let Some(size) = remote.installed_size {
        append_label(
            &mut output,
            "Installed Size:",
            Some(&format_remote_size(size)),
            styled,
        );
    }
    append_label(&mut output, "Runtime:", Some(&remote.runtime_ref), styled);
    if let Some(sdk) = &remote.sdk_ref {
        append_label(&mut output, "Sdk:", Some(sdk), styled);
    }
    output
}

fn append_commit(
    output: &mut String,
    commit: &storage::CommitInfo,
    include_parent: bool,
    styled: bool,
) {
    append_label(output, "Commit:", Some(&commit.checksum), styled);
    if include_parent {
        if let Some(parent) = &commit.parent {
            append_label(output, "Parent:", Some(parent), styled);
        }
    }
    if !commit.subject.is_empty() {
        append_label(output, "Subject:", Some(&commit.subject), styled);
    }
    if let Ok(date) = glib::DateTime::from_unix_utc(commit.timestamp as i64)
        .and_then(|date| date.format("%Y-%m-%d %H:%M:%S +0000"))
    {
        append_label(output, "Date:", Some(date.as_str()), styled);
    }
}

fn append_label(output: &mut String, label: &str, value: Option<&str>, styled: bool) {
    const WIDTH: usize = 15;
    let padding = WIDTH.saturating_sub(label.len());
    output.push_str(&" ".repeat(padding));
    if styled {
        let _ = write!(output, "\x1b[1m{label}\x1b[0m");
    } else {
        output.push_str(label);
    }
    if let Some(value) = value {
        output.push(' ');
        output.push_str(value);
    }
    output.push('\n');
}

fn format_remote_size(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.1} kB", bytes as f64 / 1_000.0)
    } else {
        format!("{bytes} bytes")
    }
}

#[derive(Debug, PartialEq, Eq)]
struct RemoteInfoOptions {
    log: bool,
    commit: Option<String>,
    app_id: String,
}

fn parse_remote_info_args(args: Vec<String>) -> Result<RemoteInfoOptions> {
    let mut log = false;
    let mut commit = None;
    let mut operands = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--log" => log = true,
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
            _ if arg.starts_with('-') => bail!("unknown remote-info option: {arg}"),
            _ => operands.push(arg),
        }
    }
    if log && commit.is_some() {
        bail!("--log and --commit cannot be used together");
    }
    if operands.len() != 2 {
        bail!("usage: flatpak remote-info [--log | --commit=COMMIT] flathub <app-id>");
    }
    if operands[0] != "flathub" {
        bail!("remote is not configured: {}", operands[0]);
    }
    Ok(RemoteInfoOptions {
        log,
        commit,
        app_id: operands.remove(1),
    })
}

fn cmd_install(paths: &Installation, args: Vec<String>) -> Result<()> {
    if args.len() != 1 {
        bail!("usage: flatpak install <app-id>");
    }
    let total_started = Instant::now();
    println!("==> Resolving {}", args[0]);
    let installed = runtime::install_app(paths, &args[0])?;
    let was_installed = state::get_app(paths, &installed.app_id).is_ok();
    let record = state::record_install(paths, &installed)?;
    println!("\n==> Publishing desktop integration");
    let export_started = Instant::now();
    let export = match desktop::export_app(paths, &record) {
        Ok(export) => export,
        Err(error) => {
            if !was_installed {
                let _ = desktop::remove_export(paths, &record.app_id);
                let _ = state::remove_app_record(paths, &record.app_id);
            }
            return Err(error).context("publish desktop integration");
        }
    };
    let export_elapsed = export_started.elapsed();
    println!("\n==> Installed {}", installed.app_id);
    println!("    Runtime: {}", installed.runtime_ref);
    println!("    Launch: flatpak run {}", installed.app_id);
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

fn cmd_list(paths: &Installation, args: Vec<String>) -> Result<()> {
    if !args.is_empty() {
        bail!("usage: flatpak list");
    }
    let apps = state::list_apps(paths)?;
    if apps.is_empty() {
        println!("No installed apps");
        return Ok(());
    }
    println!(
        "{:<34} {:<8} {:<8} {:<32} Command",
        "Application ID", "Arch", "Branch", "Runtime"
    );
    for app in apps {
        println!(
            "{:<34} {:<8} {:<8} {:<32} {}",
            app.app_id, app.arch, app.branch, app.runtime_ref, app.command
        );
    }
    Ok(())
}

fn cmd_ps(paths: &Installation, args: Vec<String>) -> Result<()> {
    print!("{}", ps::output(paths, args)?);
    Ok(())
}

fn cmd_permissions(paths: &Installation, args: Vec<String>) -> Result<()> {
    if args.len() != 1 {
        bail!("usage: flatpak permissions <app-id>");
    }
    let record = state::get_app(paths, &args[0])?;
    let user = std::env::var("USER").unwrap_or_else(|_| "user".to_string());
    let sandbox_root = paths.chroots().join(&record.app_id);
    let uid = numeric_id("id", "-u")?;
    let xdg_runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{uid}")));
    let metadata_path = state::absolute(paths, &record.app_dir).join("metadata");
    let host_filesystem = filesystem::HostFilesystem::from_metadata_file_for_user(
        &metadata_path,
        &user,
        paths.data_root(),
        &sandbox_root,
    )?;
    let host_audio = audio::HostAudio::from_metadata_file(&metadata_path, &xdg_runtime_dir, uid)?;

    println!("Filesystem permissions for {}", record.app_id);
    println!("Metadata filesystems:");
    if host_filesystem.permissions().is_empty() {
        println!("  <none>");
    } else {
        for permission in host_filesystem.permissions() {
            let create = if permission.create() { ", create" } else { "" };
            println!(
                "  {:<28} {}{}",
                permission.original(),
                permission.access().label(),
                create
            );
        }
    }

    println!("Resolved nullfs grants:");
    if host_filesystem.grants().is_empty() {
        println!("  <none>");
    } else {
        for grant in host_filesystem.grants() {
            println!(
                "  {:<42} -> {:<42} {} ({})",
                grant.host_path().display(),
                grant.sandbox_path().display(),
                grant.access().label(),
                grant.source_permission()
            );
        }
    }

    if !host_filesystem.warnings().is_empty() {
        println!("Warnings:");
        for warning in host_filesystem.warnings() {
            println!("  {warning}");
        }
    }

    println!("Socket permissions:");
    if host_audio.sockets().is_empty() {
        println!("  <none>");
    } else {
        for socket in host_audio.sockets() {
            println!("  {socket}");
        }
    }

    println!("Resolved audio bridge:");
    let audio_lines = host_audio.describe();
    if audio_lines.is_empty() {
        println!("  <none>");
    } else {
        for line in audio_lines {
            println!("  {line}");
        }
    }
    if !host_audio.warnings().is_empty() {
        println!("Audio warnings:");
        for warning in host_audio.warnings() {
            println!("  {warning}");
        }
    }

    Ok(())
}

fn cmd_run(paths: &Installation, args: Vec<String>) -> Result<()> {
    let (app_id, mut options) = parse_run_args(args)?;
    if options.app_dir.is_none() && options.runtime_dir.is_none() && options.entry.is_none() {
        let record = state::get_app(paths, &app_id)?;
        options.app_dir = Some(state::absolute(paths, &record.app_dir));
        options.runtime_dir = Some(state::absolute(paths, &record.runtime_dir));
        options.entry = Some(record.command);
    }

    let app = runtime::resolve_app(paths, &app_id, options)?;
    let desktop = desktop::DesktopSession::from_env()
        .context("XDG_RUNTIME_DIR and WAYLAND_DISPLAY must be set")?;
    let backend = sandbox::ChrootNullfsBackend::new(paths.clone());
    let status = backend.run(&app, &desktop)?;
    if !status.success() {
        anyhow::bail!("{} exited with status {}", app.app_id, status);
    }
    Ok(())
}

fn cmd_uninstall(paths: &Installation, args: Vec<String>) -> Result<()> {
    if args.len() != 1 {
        bail!("usage: flatpak uninstall <app-id>");
    }
    let app_id = &args[0];
    if sandbox::app_has_mounts(paths, app_id)? {
        bail!("{app_id} still has active sandbox mounts; stop it before uninstalling");
    }
    let record = match state::get_app(paths, app_id) {
        Ok(record) => record,
        Err(_) => {
            desktop::remove_export(paths, app_id)?;
            println!("{app_id} is not installed");
            return Ok(());
        }
    };
    desktop::remove_export(paths, app_id)?;
    if state::remove_app_record(paths, app_id)?.is_none() {
        println!("{app_id} is not installed");
        return Ok(());
    }

    state::safe_remove_dir(paths, &record.app_dir)?;
    state::safe_remove_dir(paths, &paths.chroots().join(app_id))?;

    if state::runtime_is_required(paths, &record.runtime_ref)? {
        runtime::remove_repo_refs(paths, &[&record.app_ref])?;
        println!("Uninstalled {app_id}");
        println!("  kept shared runtime {}", record.runtime_ref);
    } else {
        state::safe_remove_dir(paths, &record.runtime_dir)?;
        state::remove_runtime_record(paths, &record.runtime_ref)?;
        let runtime_ref = format!("runtime/{}", record.runtime_ref);
        runtime::remove_repo_refs(paths, &[&record.app_ref, &runtime_ref])?;
        println!("Uninstalled {app_id}");
        println!("  removed unused runtime {}", record.runtime_ref);
    }
    Ok(())
}

fn cmd_repair(paths: &Installation, args: Vec<String>) -> Result<()> {
    if !args.is_empty() {
        bail!("usage: flatpak repair");
    }
    println!("Checking OSTree object integrity...");
    let checked = runtime::repair_repo(paths)?;
    println!("Checked {checked} objects; no corruption found");
    Ok(())
}

fn cmd_prune(paths: &Installation, args: Vec<String>) -> Result<()> {
    if !args.is_empty() {
        bail!("usage: flatpak prune");
    }
    let (total, pruned, bytes) = runtime::prune_repo(paths)?;
    println!("Pruned {pruned} of {total} objects ({bytes} bytes reclaimed)");
    Ok(())
}

fn cmd_update(paths: &Installation, args: Vec<String>) -> Result<()> {
    let options = parse_update_args(args)?;
    let installed = state::list_apps(paths)?;
    if installed.is_empty() {
        if let Some(app_id) = options.app_ids.first() {
            bail!("{app_id} is not installed");
        }
        println!("No installed apps");
        return Ok(());
    }
    let metadata = runtime::load_remote_metadata(paths)?;
    let targets = update_targets(installed, options.app_ids, &metadata)?;

    let mut touched_runtimes = BTreeSet::new();
    for target in targets {
        let record = target.record;
        if sandbox::app_has_mounts(paths, &record.app_id)? {
            bail!(
                "{} still has active sandbox mounts; stop it before updating",
                record.app_id
            );
        }

        let remote = if let Some(commit) = options.commit.as_deref() {
            metadata.resolve_app_commit(paths, &record.app_ref, commit)?
        } else {
            match target.remote {
                Some(remote) => remote,
                None => metadata
                    .resolve_exact_ref(&record.app_ref)
                    .or_else(|_| metadata.resolve_app(&record.app_id, true))?,
            }
        };
        let status = update_status(paths, &record, &remote)?;

        if !status.app_changed && !status.runtime_changed {
            println!("{} is up to date", record.app_id);
            continue;
        }

        let force_runtime =
            status.runtime_checkout_stale && touched_runtimes.insert(remote.runtime_ref.clone());
        let installed =
            runtime::update_app(paths, &remote, status.app_checkout_stale, force_runtime)?;
        let installed_record = state::record_install(paths, &installed)?;
        if record.app_id != installed.app_id {
            desktop::remove_export(paths, &record.app_id)?;
            state::remove_app_record(paths, &record.app_id)?;
            state::safe_remove_dir(paths, &record.app_dir)?;
        }
        let export = desktop::export_app(paths, &installed_record)?;
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

        if record.runtime_ref != installed.runtime_ref
            && !state::runtime_is_required(paths, &record.runtime_ref)?
        {
            state::safe_remove_dir(paths, &record.runtime_dir)?;
            state::remove_runtime_record(paths, &record.runtime_ref)?;
        }
    }

    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct UpdateOptions {
    commit: Option<String>,
    app_ids: Vec<String>,
}

fn parse_update_args(args: Vec<String>) -> Result<UpdateOptions> {
    let mut commit = None;
    let mut app_ids = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
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
    Ok(UpdateOptions { commit, app_ids })
}

#[derive(Debug)]
struct UpdateTarget {
    record: state::AppRecord,
    remote: Option<runtime::RemoteApp>,
}

#[derive(Debug, PartialEq, Eq)]
struct UpdateStatus {
    app_changed: bool,
    app_checkout_stale: bool,
    runtime_changed: bool,
    runtime_checkout_stale: bool,
    current_runtime_commit: Option<String>,
}

fn update_targets(
    installed: Vec<state::AppRecord>,
    args: Vec<String>,
    metadata: &runtime::RemoteMetadata,
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

fn update_status(
    paths: &Installation,
    record: &state::AppRecord,
    remote: &runtime::RemoteApp,
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

    let runtime_dir = paths
        .runtimes()
        .join(runtime::runtime_checkout_dir(&remote.runtime_ref));
    let current_runtime_commit = state::runtime_commit(paths, &remote.runtime_ref)?.or_else(|| {
        if record.runtime_ref == remote.runtime_ref {
            Some(record.runtime_commit.clone())
        } else {
            None
        }
    });
    let runtime_checkout_stale = !checkout_present(&runtime_dir)
        || current_runtime_commit.as_deref() != Some(remote.runtime_commit.as_str());
    let runtime_state_changed =
        record.runtime_ref != remote.runtime_ref || record.runtime_commit != remote.runtime_commit;

    Ok(UpdateStatus {
        app_changed: app_state_changed || app_checkout_stale,
        app_checkout_stale,
        runtime_changed: runtime_state_changed || runtime_checkout_stale,
        runtime_checkout_stale,
        current_runtime_commit,
    })
}

fn checkout_present(dir: &Path) -> bool {
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

fn parse_run_args(args: Vec<String>) -> Result<(String, runtime::ResolveAppOptions)> {
    let mut args = args.into_iter();
    let app_id = args.next().context("missing app id")?;
    let mut options = runtime::ResolveAppOptions::default();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--" => {
                options.args.extend(args);
                break;
            }
            "--app-dir" => options.app_dir = Some(next_path(&mut args, "--app-dir")?),
            "--runtime-dir" => options.runtime_dir = Some(next_path(&mut args, "--runtime-dir")?),
            "--entry" => options.entry = Some(args.next().context("missing value for --entry")?),
            _ if arg.starts_with("--app-dir=") => {
                options.app_dir = Some(PathBuf::from(value_after_equals(&arg)))
            }
            _ if arg.starts_with("--runtime-dir=") => {
                options.runtime_dir = Some(PathBuf::from(value_after_equals(&arg)))
            }
            _ if arg.starts_with("--entry=") => {
                options.entry = Some(value_after_equals(&arg).to_string())
            }
            _ => {
                options.args.push(arg);
                options.args.extend(args);
                break;
            }
        }
    }

    Ok((app_id, options))
}

fn next_path(args: &mut impl Iterator<Item = String>, option: &str) -> Result<PathBuf> {
    Ok(Path::new(
        &args
            .next()
            .with_context(|| format!("missing value for {option}"))?,
    )
    .into())
}

fn value_after_equals(arg: &str) -> &str {
    arg.split_once('=').map(|(_, value)| value).unwrap_or("")
}

fn numeric_id(program: &str, arg: &str) -> Result<u32> {
    let output = std::process::Command::new(program)
        .arg(arg)
        .output()
        .with_context(|| format!("run {program} {arg}"))?;
    if !output.status.success() {
        bail!("{program} {arg} failed with status {}", output.status);
    }
    let text = String::from_utf8(output.stdout)?.trim().to_string();
    text.parse::<u32>()
        .with_context(|| format!("parse numeric id from {text:?}"))
}

fn print_usage() {
    eprintln!("usage:");
    eprintln!("  flatpak search <query>");
    eprintln!("  flatpak install <app-id>");
    eprintln!("  flatpak remote-info [--log | --commit=COMMIT] flathub <app-id>");
    eprintln!("  flatpak list");
    eprintln!("  flatpak permissions <app-id>");
    eprintln!("  flatpak ps [--columns=FIELD,...]");
    eprintln!("  flatpak prune");
    eprintln!("  flatpak repair");
    eprintln!("  flatpak run <app-id> [-- app-args...]");
    eprintln!("  flatpak uninstall <app-id>");
    eprintln!("  flatpak update [--commit=COMMIT] [app-id...]");
}

fn print_help() {
    print!("{HELP}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_ID: AtomicUsize = AtomicUsize::new(0);

    fn test_dir(name: &str) -> PathBuf {
        let id = TEST_ID.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "freebsd-flatpak-poc-main-{name}-{}-{id}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn create_checkout(root: &Path, rel: &Path) {
        let dir = root.join(rel);
        fs::create_dir_all(dir.join("files")).unwrap();
        fs::write(
            dir.join("metadata"),
            "[Application]\nname=org.example.App\n",
        )
        .unwrap();
    }

    fn app_record(app_id: &str, app_ref: &str, app_commit: &str) -> state::AppRecord {
        state::AppRecord {
            app_id: app_id.to_string(),
            app_ref: app_ref.to_string(),
            app_commit: app_commit.to_string(),
            app_dir: PathBuf::from("apps").join(app_id),
            arch: "x86_64".to_string(),
            branch: "stable".to_string(),
            runtime_ref: "org.example.Platform/x86_64/stable".to_string(),
            runtime_commit: "runtime-1".to_string(),
            runtime_dir: PathBuf::from("runtimes").join("org.example.Platform-stable"),
            command: "old-command".to_string(),
        }
    }

    fn remote_app(app_id: &str, app_ref: &str, app_commit: &str) -> runtime::RemoteApp {
        runtime::RemoteApp {
            app_id: app_id.to_string(),
            app_ref: app_ref.to_string(),
            app_commit: app_commit.to_string(),
            arch: "x86_64".to_string(),
            branch: "stable".to_string(),
            runtime_ref: "org.example.Platform/x86_64/stable".to_string(),
            runtime_commit: "runtime-1".to_string(),
            sdk_ref: None,
            download_size: None,
            installed_size: None,
            command: "new-command".to_string(),
        }
    }

    fn create_runtime_checkout(paths: &Installation) {
        create_checkout(
            paths.data_root(),
            &PathBuf::from("runtimes").join("org.example.Platform-stable"),
        );
    }

    #[test]
    fn newer_remote_app_commit_requires_app_checkout() {
        let root = test_dir("newer-app-commit");
        let paths = Installation::for_test(&root);
        create_checkout(
            paths.data_root(),
            &PathBuf::from("apps").join("org.example.App"),
        );
        create_runtime_checkout(&paths);
        let mut record = app_record(
            "org.example.App",
            "app/org.example.App/x86_64/stable",
            "app-1",
        );
        record.command = "new-command".to_string();
        let remote = remote_app(
            "org.example.App",
            "app/org.example.App/x86_64/stable",
            "app-2",
        );

        let status = update_status(&paths, &record, &remote).unwrap();

        assert!(status.app_changed);
        assert!(status.app_checkout_stale);
        assert!(!status.runtime_changed);
        assert!(!status.runtime_checkout_stale);
    }

    #[test]
    fn app_id_replacement_requires_app_checkout_even_with_same_commit() {
        let root = test_dir("replacement");
        let paths = Installation::for_test(&root);
        create_checkout(
            paths.data_root(),
            &PathBuf::from("apps").join("org.example.OldApp"),
        );
        create_runtime_checkout(&paths);
        let mut record = app_record(
            "org.example.OldApp",
            "app/org.example.OldApp/x86_64/stable",
            "app-1",
        );
        record.command = "new-command".to_string();
        let remote = remote_app(
            "org.example.NewApp",
            "app/org.example.NewApp/x86_64/stable",
            "app-1",
        );

        let status = update_status(&paths, &record, &remote).unwrap();

        assert!(status.app_changed);
        assert!(status.app_checkout_stale);
    }

    #[test]
    fn missing_runtime_checkout_requires_runtime_checkout() {
        let root = test_dir("missing-runtime");
        let paths = Installation::for_test(&root);
        create_checkout(
            paths.data_root(),
            &PathBuf::from("apps").join("org.example.App"),
        );
        let mut record = app_record(
            "org.example.App",
            "app/org.example.App/x86_64/stable",
            "app-1",
        );
        record.command = "new-command".to_string();
        let remote = remote_app(
            "org.example.App",
            "app/org.example.App/x86_64/stable",
            "app-1",
        );

        let status = update_status(&paths, &record, &remote).unwrap();

        assert!(status.runtime_changed);
        assert!(status.runtime_checkout_stale);
        assert_eq!(status.current_runtime_commit.as_deref(), Some("runtime-1"));
    }

    #[test]
    fn stale_record_command_updates_state_without_app_checkout() {
        let root = test_dir("state-only-command");
        let paths = Installation::for_test(&root);
        create_checkout(
            paths.data_root(),
            &PathBuf::from("apps").join("org.example.App"),
        );
        create_runtime_checkout(&paths);
        let record = app_record(
            "org.example.App",
            "app/org.example.App/x86_64/stable",
            "app-1",
        );
        let remote = remote_app(
            "org.example.App",
            "app/org.example.App/x86_64/stable",
            "app-1",
        );

        let status = update_status(&paths, &record, &remote).unwrap();

        assert!(status.app_changed);
        assert!(!status.app_checkout_stale);
    }

    #[test]
    fn older_remote_app_commit_requires_app_checkout_for_downgrade() {
        let root = test_dir("older-app-commit");
        let paths = Installation::for_test(&root);
        create_checkout(
            paths.data_root(),
            &PathBuf::from("apps").join("org.example.App"),
        );
        create_runtime_checkout(&paths);
        let mut record = app_record(
            "org.example.App",
            "app/org.example.App/x86_64/stable",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        );
        record.command = "new-command".to_string();
        let remote = remote_app(
            "org.example.App",
            "app/org.example.App/x86_64/stable",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );

        let status = update_status(&paths, &record, &remote).unwrap();

        assert!(status.app_changed);
        assert!(status.app_checkout_stale);
    }

    #[test]
    fn update_commit_requires_exactly_one_app() {
        assert_eq!(
            parse_update_args(vec![
                "--commit=abc123".to_string(),
                "org.example.App".to_string()
            ])
            .unwrap(),
            UpdateOptions {
                commit: Some("abc123".to_string()),
                app_ids: vec!["org.example.App".to_string()],
            }
        );
        assert!(parse_update_args(vec!["--commit=abc123".to_string()]).is_err());
        assert!(parse_update_args(vec![
            "--commit=abc123".to_string(),
            "org.example.One".to_string(),
            "org.example.Two".to_string(),
        ])
        .is_err());
    }

    #[test]
    fn remote_info_parses_log_and_historical_commit_modes() {
        assert_eq!(
            parse_remote_info_args(vec![
                "--log".to_string(),
                "flathub".to_string(),
                "org.example.App".to_string(),
            ])
            .unwrap(),
            RemoteInfoOptions {
                log: true,
                commit: None,
                app_id: "org.example.App".to_string(),
            }
        );
        assert_eq!(
            parse_remote_info_args(vec![
                "--commit=abc123".to_string(),
                "flathub".to_string(),
                "org.example.App".to_string(),
            ])
            .unwrap()
            .commit
            .as_deref(),
            Some("abc123")
        );
        assert!(parse_remote_info_args(vec![
            "--log".to_string(),
            "--commit=abc123".to_string(),
            "flathub".to_string(),
            "org.example.App".to_string(),
        ])
        .is_err());
    }

    fn history_commit(checksum: &str, parent: Option<&str>, subject: &str) -> storage::CommitInfo {
        storage::CommitInfo {
            checksum: checksum.to_string(),
            parent: parent.map(ToString::to_string),
            timestamp: 0,
            subject: subject.to_string(),
            body: String::new(),
            flatpak_metadata: None,
            version: None,
            collection_id: None,
        }
    }

    #[test]
    fn remote_log_matches_flatpak_metadata_and_history_structure() {
        let mut remote = remote_app(
            "org.example.App",
            "app/org.example.App/x86_64/stable",
            "tip",
        );
        remote.runtime_ref = "org.example.Platform/x86_64/50".to_string();
        remote.sdk_ref = Some("org.example.Sdk/x86_64/50".to_string());
        remote.download_size = Some(1_800_000);
        remote.installed_size = Some(4_700_000);
        let history = vec![
            history_commit("tip", Some("old"), "Current build"),
            history_commit("old", Some("unavailable"), "Older build"),
        ];
        let appstream = runtime::AppstreamInfo {
            name: Some("Example".to_string()),
            summary: Some("Do useful things".to_string()),
            version: Some("50.0".to_string()),
            license: Some("GPL-3.0-or-later".to_string()),
        };

        assert_eq!(
            remote_log_output(
                &remote,
                &history,
                Some(&appstream),
                Some("org.example.Stable"),
                false,
            ),
            concat!(
                "\n",
                "Example - Do useful things\n",
                "\n",
                "            ID: org.example.App\n",
                "           Ref: app/org.example.App/x86_64/stable\n",
                "          Arch: x86_64\n",
                "        Branch: stable\n",
                "       Version: 50.0\n",
                "       License: GPL-3.0-or-later\n",
                "    Collection: org.example.Stable\n",
                " Download Size: 1.8 MB\n",
                "Installed Size: 4.7 MB\n",
                "       Runtime: org.example.Platform/x86_64/50\n",
                "           Sdk: org.example.Sdk/x86_64/50\n",
                "\n",
                "        Commit: tip\n",
                "        Parent: old\n",
                "       Subject: Current build\n",
                "          Date: 1970-01-01 00:00:00 +0000\n",
                "       History:\n",
                "\n",
                "        Commit: old\n",
                "       Subject: Older build\n",
                "          Date: 1970-01-01 00:00:00 +0000\n",
            )
        );
    }

    #[test]
    fn remote_info_bolds_labels_only_when_styled() {
        let remote = remote_app(
            "org.example.App",
            "app/org.example.App/x86_64/stable",
            "tip",
        );

        let plain = remote_info_output(&remote, None, None, None, false, false);
        let styled = remote_info_output(&remote, None, None, None, false, true);

        assert!(!plain.contains("\x1b["));
        assert!(styled.contains("\x1b[1mID:\x1b[0m org.example.App"));
        assert!(styled.contains("\x1b[1mCommit:\x1b[0m tip"));
        assert!(!styled.contains("\x1b[1morg.example.App"));
    }

    #[test]
    fn remote_info_omits_header_and_optional_fields_when_metadata_is_missing() {
        let remote = remote_app(
            "org.gnome.Calculator",
            "app/org.gnome.Calculator/x86_64/stable",
            "tip",
        );

        let output = remote_info_output(&remote, None, None, None, false, false);
        assert!(output.starts_with("\n            ID: org.gnome.Calculator\n"));
        assert!(!output.contains("Calculator\n\n"));
        assert!(!output.contains("Version:"));
        assert!(!output.contains("License:"));
        assert!(!output.contains("Collection:"));
    }

    #[test]
    fn historical_remote_info_uses_commit_version_and_collection_only() {
        let remote = remote_app(
            "org.example.App",
            "app/org.example.App/x86_64/stable",
            "old",
        );
        let appstream = runtime::AppstreamInfo {
            name: Some("Example".to_string()),
            summary: Some("Do useful things".to_string()),
            version: Some("current-version".to_string()),
            license: Some("current-license".to_string()),
        };
        let mut commit = history_commit("old", Some("older"), "Old build");
        commit.version = Some("historical-version".to_string());
        commit.collection_id = Some("org.example.Historical".to_string());

        let output = remote_info_output(
            &remote,
            Some(&commit),
            Some(&appstream),
            Some("org.example.Current"),
            true,
            false,
        );

        assert!(output.contains("Version: historical-version"));
        assert!(output.contains("Collection: org.example.Historical"));
        assert!(!output.contains("current-version"));
        assert!(!output.contains("current-license"));
        assert!(!output.contains("org.example.Current"));
    }
}
