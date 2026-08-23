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
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::{BufRead, IsTerminal, Write};
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

const UNINSTALL_HELP: &str = r#"Usage:
  flatpak uninstall [OPTION] [APP-ID]

Options:
  --unused             Remove unused runtime and extension refs
  --delete-data        Delete app data
  -y, --assumeyes      Automatically answer yes for all questions
  --noninteractive     Produce minimal output and don't ask questions
  -h, --help           Show help
"#;

const INSTALL_HELP: &str = r#"Usage:
  flatpak install [OPTION] APP-ID

Options:
  --or-update          Update install if already installed
  -y, --assumeyes      Automatically answer yes for all questions
  --noninteractive     Produce minimal output and don't ask questions
  -h, --help           Show help
"#;

const UPDATE_HELP: &str = r#"Usage:
  flatpak update [OPTION] [APP-ID...]

Options:
  --commit=COMMIT      Update to this commit
  -y, --assumeyes      Automatically answer yes for all questions
  --noninteractive     Produce minimal output and don't ask questions
  -h, --help           Show help
"#;

fn main() -> Result<()> {
    let all_args = std::env::args().skip(1).collect::<Vec<_>>();
    let mut args = all_args.clone().into_iter();
    let command = args.next();
    if matches!(command.as_deref(), Some("-h" | "--help")) {
        print_help();
        return Ok(());
    }
    let command_args = all_args.get(1..).unwrap_or_default();
    if command_args == ["-h"] || command_args == ["--help"] {
        match command.as_deref() {
            Some("install") => print_install_help(),
            Some("update" | "upgrade") => print_update_help(),
            Some("uninstall" | "remove") => print_uninstall_help(),
            _ => {}
        }
        if matches!(
            command.as_deref(),
            Some("install" | "update" | "upgrade" | "uninstall" | "remove")
        ) {
            return Ok(());
        }
    }

    let paths = Installation::from_env()?;
    state::ensure_layout(&paths)?;
    sandbox::recover_stale_mounts(&paths)?;
    portal::recover_stale_portal_mounts(&paths)?;
    graphics::recover_stale_graphics_dirs(&paths)?;
    runtime::recover_storage(&paths)?;
    state::reconcile_runtime_bindings(&paths)?;
    state::cleanup_retired_deployments(&paths)?;

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
        Some("uninstall" | "remove") => cmd_uninstall(&paths, args.collect()),
        Some("update" | "upgrade") => cmd_update(&paths, args.collect()),
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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct TransactionOptions {
    assumeyes: bool,
    noninteractive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransactionOperation {
    Install,
    Update,
    Uninstall,
}

impl TransactionOperation {
    fn label(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Update => "update",
            Self::Uninstall => "uninstall",
        }
    }

    fn active_label(self) -> &'static str {
        match self {
            Self::Install => "Installing",
            Self::Update => "Updating",
            Self::Uninstall => "Uninstalling",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TransactionEntry {
    operation: TransactionOperation,
    kind: &'static str,
    ref_name: String,
}

fn present_and_confirm(entries: &[TransactionEntry], options: TransactionOptions) -> Result<bool> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    present_and_confirm_with(entries, options, &mut stdin.lock(), &mut stdout.lock())
}

fn present_and_confirm_with(
    entries: &[TransactionEntry],
    options: TransactionOptions,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<bool> {
    if options.noninteractive {
        for entry in entries {
            writeln!(
                output,
                "{} {}",
                entry.operation.active_label(),
                entry.ref_name
            )?;
        }
        return Ok(true);
    }

    writeln!(output, "\nChanges:")?;
    writeln!(output, "  {:<11} {:<10} Ref", "Operation", "Type")?;
    for entry in entries {
        writeln!(
            output,
            "  {:<11} {:<10} {}",
            entry.operation.label(),
            entry.kind,
            entry.ref_name
        )?;
    }
    if options.assumeyes {
        return Ok(true);
    }

    loop {
        write!(output, "\nProceed with these changes? [Y/n]: ")?;
        output.flush()?;
        let mut answer = String::new();
        if input.read_line(&mut answer)? == 0 {
            writeln!(output, "Cancelled.")?;
            return Ok(false);
        }
        match answer.trim().to_ascii_lowercase().as_str() {
            "" | "y" | "yes" => return Ok(true),
            "n" | "no" => {
                writeln!(output, "Cancelled.")?;
                return Ok(false);
            }
            _ => writeln!(output, "Please answer y or n.")?,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct InstallOptions {
    transaction: TransactionOptions,
    or_update: bool,
    app_id: String,
}

fn parse_install_args(args: Vec<String>) -> Result<InstallOptions> {
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
    if operands.len() != 1 {
        bail!("usage: flatpak install [OPTION] <app-id>");
    }
    Ok(InstallOptions {
        transaction,
        or_update,
        app_id: operands.remove(0),
    })
}

fn cmd_install(paths: &Installation, args: Vec<String>) -> Result<()> {
    let options = parse_install_args(args)?;
    let total_started = Instant::now();
    if !options.transaction.noninteractive {
        println!("==> Resolving {}", options.app_id);
    }
    let resolution_started = Instant::now();
    let remote = runtime::resolve_remote_app(paths, &options.app_id)?;
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

    let runtime_record = state::get_runtime(paths, &remote.runtime_ref)?;
    let runtime_dir = runtime_record
        .as_ref()
        .map(|record| state::absolute(paths, &record.runtime_dir))
        .unwrap_or_else(|| {
            paths
                .runtimes()
                .join(runtime::runtime_checkout_dir(&remote.runtime_ref))
        });
    let runtime_changed = runtime_record
        .as_ref()
        .map(|record| record.runtime_commit.as_str())
        != Some(remote.runtime_commit.as_str())
        || !checkout_present(&runtime_dir);
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
    let export = match desktop::export_app(paths, &record) {
        Ok(export) => export,
        Err(error) => {
            let _ = desktop::remove_export(paths, &record.app_id);
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

#[derive(Debug, PartialEq, Eq)]
struct UninstallOptions {
    transaction: TransactionOptions,
    unused: bool,
    delete_data: bool,
    app_id: Option<String>,
}

fn parse_uninstall_args(args: Vec<String>) -> Result<UninstallOptions> {
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

fn cmd_uninstall(paths: &Installation, args: Vec<String>) -> Result<()> {
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
    desktop::remove_export(paths, app_id)?;
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

fn remove_app_data(paths: &Installation, app_id: &str) -> Result<()> {
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
struct UnusedRemoval {
    ref_name: String,
    kind: &'static str,
    deployment_paths: BTreeSet<PathBuf>,
    runtime_ref: Option<String>,
}

fn plan_unused_deployment_checkouts(paths: &Installation) -> Result<Vec<UnusedRemoval>> {
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

fn apply_unused_deployment_plan(
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
fn remove_unused_deployment_checkouts(paths: &Installation) -> Result<Vec<String>> {
    let plan = plan_unused_deployment_checkouts(paths)?;
    apply_unused_deployment_plan(paths, plan)
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
    remote: runtime::RemoteApp,
    status: UpdateStatus,
}

fn update_resolved(
    paths: &Installation,
    resolved: Vec<(state::AppRecord, runtime::RemoteApp)>,
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
struct UpdateOptions {
    transaction: TransactionOptions,
    commit: Option<String>,
    app_ids: Vec<String>,
}

fn parse_update_args(args: Vec<String>) -> Result<UpdateOptions> {
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
    eprintln!("  flatpak install [OPTION] <app-id>");
    eprintln!("  flatpak remote-info [--log | --commit=COMMIT] flathub <app-id>");
    eprintln!("  flatpak list");
    eprintln!("  flatpak permissions <app-id>");
    eprintln!("  flatpak ps [--columns=FIELD,...]");
    eprintln!("  flatpak prune");
    eprintln!("  flatpak repair");
    eprintln!("  flatpak run <app-id> [-- app-args...]");
    eprintln!("  flatpak uninstall [OPTION] [--unused | <app-id>]");
    eprintln!("  flatpak update [OPTION] [app-id...]");
}

fn print_help() {
    print!("{HELP}");
}

fn print_uninstall_help() {
    print!("{UNINSTALL_HELP}");
}

fn print_install_help() {
    print!("{INSTALL_HELP}");
}

fn print_update_help() {
    print!("{UPDATE_HELP}");
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

    fn create_marked_checkout(path: &Path, ref_name: &str, commit: &str, metadata: &str) {
        fs::create_dir_all(path.join("files")).unwrap();
        fs::write(path.join("metadata"), metadata).unwrap();
        fs::write(
            path.join(".ostree-commit"),
            format!("{ref_name}\n{commit}\n"),
        )
        .unwrap();
    }

    fn transaction_entry() -> TransactionEntry {
        TransactionEntry {
            operation: TransactionOperation::Install,
            kind: "application",
            ref_name: "app/org.example.App/x86_64/stable".to_string(),
        }
    }

    #[test]
    fn transaction_confirmation_accepts_enter_and_explicit_yes() {
        for answer in ["\n", "y\n", "Y\n"] {
            let mut input = std::io::Cursor::new(answer.as_bytes());
            let mut output = Vec::new();
            assert!(present_and_confirm_with(
                &[transaction_entry()],
                TransactionOptions::default(),
                &mut input,
                &mut output,
            )
            .unwrap());
            let output = String::from_utf8(output).unwrap();
            assert!(output.contains("Changes:"));
            assert!(output.contains("Proceed with these changes? [Y/n]:"));
        }
    }

    #[test]
    fn transaction_confirmation_no_and_eof_cancel_cleanly() {
        for answer in ["n\n", "N\n", ""] {
            let mut input = std::io::Cursor::new(answer.as_bytes());
            let mut output = Vec::new();
            assert!(!present_and_confirm_with(
                &[transaction_entry()],
                TransactionOptions::default(),
                &mut input,
                &mut output,
            )
            .unwrap());
            assert!(String::from_utf8(output).unwrap().contains("Cancelled."));
        }
    }

    #[test]
    fn assumeyes_keeps_preview_while_noninteractive_is_quiet() {
        let mut empty = std::io::Cursor::new(Vec::<u8>::new());
        let mut output = Vec::new();
        assert!(present_and_confirm_with(
            &[transaction_entry()],
            TransactionOptions {
                assumeyes: true,
                noninteractive: false,
            },
            &mut empty,
            &mut output,
        )
        .unwrap());
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("Changes:"));
        assert!(!output.contains("Proceed with"));

        let mut empty = std::io::Cursor::new(Vec::<u8>::new());
        let mut output = Vec::new();
        assert!(present_and_confirm_with(
            &[transaction_entry()],
            TransactionOptions {
                assumeyes: false,
                noninteractive: true,
            },
            &mut empty,
            &mut output,
        )
        .unwrap());
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "Installing app/org.example.App/x86_64/stable\n"
        );
    }

    #[test]
    fn transaction_flags_and_special_options_parse_together() {
        let install = parse_install_args(vec![
            "--or-update".to_string(),
            "-y".to_string(),
            "org.example.App".to_string(),
        ])
        .unwrap();
        assert!(install.or_update);
        assert!(install.transaction.assumeyes);

        let update = parse_update_args(vec![
            "--noninteractive".to_string(),
            "org.example.App".to_string(),
        ])
        .unwrap();
        assert!(update.transaction.noninteractive);

        let uninstall = parse_uninstall_args(vec![
            "--delete-data".to_string(),
            "--assumeyes".to_string(),
            "org.example.App".to_string(),
        ])
        .unwrap();
        assert!(uninstall.delete_data);
        assert!(uninstall.transaction.assumeyes);
        assert!(
            parse_uninstall_args(vec!["--unused".to_string(), "--delete-data".to_string(),])
                .is_err()
        );
    }

    #[test]
    fn delete_data_removes_only_the_requested_apps_persistent_directory() {
        let root = test_dir("delete-data");
        let paths = Installation::for_test(&root);
        state::ensure_layout(&paths).unwrap();
        let requested = paths.app_data("org.example.App").unwrap();
        let other = paths.app_data("org.example.Other").unwrap();
        fs::create_dir_all(&requested).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::write(requested.join("settings"), "app").unwrap();
        fs::write(other.join("settings"), "other").unwrap();

        remove_app_data(&paths, "org.example.App").unwrap();

        assert!(!requested.exists());
        assert_eq!(fs::read_to_string(other.join("settings")).unwrap(), "other");
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
    fn active_run_does_not_block_noop_status_or_unrelated_target_selection() {
        let root = test_dir("active-noop-unrelated");
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
        state::write_run_record(
            &paths,
            &record.app_id,
            "active",
            &paths.chroots().join("active"),
            std::process::id(),
            0,
        )
        .unwrap();
        let remote = remote_app(&record.app_id, &record.app_ref, &record.app_commit);
        let status = update_status(&paths, &record, &remote).unwrap();
        assert!(!status.app_changed);
        assert!(!status.runtime_changed);

        let other = app_record(
            "org.example.Other",
            "app/org.example.Other/x86_64/stable",
            "other-1",
        );
        let metadata = runtime::RemoteMetadata::empty_for_test(&root);
        let selected = update_targets(
            vec![record, other.clone()],
            vec![other.app_id.clone()],
            &metadata,
        )
        .unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].record.app_id, other.app_id);
    }

    #[test]
    fn uninstall_unused_preserves_installed_and_pinned_dependencies() {
        let root = test_dir("unused");
        let paths = Installation::for_test(&root);
        state::ensure_layout(&paths).unwrap();
        let runtime_one = "org.example.Platform/x86_64/one";
        let runtime_two = "org.example.Platform/x86_64/two";
        let runtime_three = "org.example.Platform/x86_64/three";
        let app_dir = paths.app("org.example.App").join("app-current");
        let runtime_one_dir = paths.runtimes().join("platform-one").join("runtime-one");
        let app_metadata = format!(
            "[Application]\nname=org.example.App\nruntime={runtime_one}\ncommand=example\n\n[Extension org.example.Keep]\ndirectory=lib/keep\nversion=one\n"
        );
        create_marked_checkout(
            &app_dir,
            "app/org.example.App/x86_64/stable",
            "app-current",
            &app_metadata,
        );
        create_marked_checkout(
            &runtime_one_dir,
            &format!("runtime/{runtime_one}"),
            "runtime-one",
            "[Runtime]\nname=org.example.Platform\n\n[Extension org.freedesktop.Platform.GL]\ndirectory=lib/x86_64-linux-gnu/GL\nversions=one;one-extra;1.4\nsubdirectories=true\ndownload-if=active-gl-driver\nenable-if=active-gl-driver\nautoprune-unless=active-gl-driver\n",
        );
        state::record_install(
            &paths,
            &runtime::InstalledApp {
                app_id: "org.example.App".to_string(),
                app_ref: "app/org.example.App/x86_64/stable".to_string(),
                app_commit: "app-current".to_string(),
                app_dir: app_dir.clone(),
                arch: "x86_64".to_string(),
                branch: "stable".to_string(),
                runtime_ref: runtime_one.to_string(),
                runtime_commit: "runtime-one".to_string(),
                runtime_dir: runtime_one_dir.clone(),
                command: "example".to_string(),
                timings: Default::default(),
            },
        )
        .unwrap();

        let pinned_app_dir = paths.app("org.example.Old").join("app-old");
        let runtime_two_dir = paths.runtimes().join("platform-two").join("runtime-two");
        create_marked_checkout(
            &pinned_app_dir,
            "app/org.example.Old/x86_64/stable",
            "app-old",
            &format!(
                "[Application]\nname=org.example.Old\nruntime={runtime_two}\ncommand=old\n\n[Extension org.example.Active]\ndirectory=lib/active\nversion=two\n"
            ),
        );
        create_marked_checkout(
            &runtime_two_dir,
            &format!("runtime/{runtime_two}"),
            "runtime-two",
            "[Runtime]\nname=org.example.Platform\n",
        );
        let pinned = state::AppRecord {
            app_id: "org.example.Old".to_string(),
            app_ref: "app/org.example.Old/x86_64/stable".to_string(),
            app_commit: "app-old".to_string(),
            app_dir: paths.relative_data_path(&pinned_app_dir).unwrap(),
            arch: "x86_64".to_string(),
            branch: "stable".to_string(),
            runtime_ref: runtime_two.to_string(),
            runtime_commit: "runtime-two".to_string(),
            runtime_dir: paths.relative_data_path(&runtime_two_dir).unwrap(),
            command: "old".to_string(),
        };
        state::write_runtime(
            &paths,
            &state::RuntimeRecord {
                runtime_ref: runtime_two.to_string(),
                runtime_commit: "runtime-two".to_string(),
                runtime_dir: pinned.runtime_dir.clone(),
            },
        )
        .unwrap();
        state::write_pinned_run_record_with_extensions(
            &paths,
            "active-old",
            &paths.chroots().join("active-old"),
            std::process::id(),
            0,
            &pinned,
            &["runtime/org.example.PinnedOnly/x86_64/two".to_string()],
        )
        .unwrap();

        let runtime_three_dir = paths
            .runtimes()
            .join("platform-three")
            .join("runtime-three");
        create_marked_checkout(
            &runtime_three_dir,
            &format!("runtime/{runtime_three}"),
            "runtime-three",
            "[Runtime]\nname=org.example.Platform\n",
        );
        state::write_runtime(
            &paths,
            &state::RuntimeRecord {
                runtime_ref: runtime_three.to_string(),
                runtime_commit: "runtime-three".to_string(),
                runtime_dir: paths.relative_data_path(&runtime_three_dir).unwrap(),
            },
        )
        .unwrap();

        for (name, ref_name) in [
            ("keep", "runtime/org.example.Keep/x86_64/one"),
            ("active", "runtime/org.example.Active/x86_64/two"),
            (
                "gl-default",
                "runtime/org.freedesktop.Platform.GL.default/x86_64/one",
            ),
            ("pinned-only", "runtime/org.example.PinnedOnly/x86_64/two"),
            ("unused", "runtime/org.example.Unused/x86_64/one"),
        ] {
            create_marked_checkout(
                &paths.extensions().join(name),
                ref_name,
                name,
                "[Runtime]\nname=extension\n",
            );
        }

        let plan = plan_unused_deployment_checkouts(&paths).unwrap();
        let planned_refs = plan
            .iter()
            .map(|item| item.ref_name.clone())
            .collect::<BTreeSet<_>>();
        assert!(planned_refs.contains(&format!("runtime/{runtime_three}")));
        assert!(planned_refs.contains("runtime/org.example.Unused/x86_64/one"));
        assert!(runtime_three_dir.exists());
        assert!(paths.extensions().join("unused").exists());

        let removed = apply_unused_deployment_plan(&paths, plan).unwrap();
        assert!(removed.contains(&format!("runtime/{runtime_three}")));
        assert!(removed.contains(&"runtime/org.example.Unused/x86_64/one".to_string()));
        assert!(runtime_one_dir.exists());
        assert!(runtime_two_dir.exists());
        assert!(!runtime_three_dir.exists());
        assert!(paths.extensions().join("keep").exists());
        assert!(paths.extensions().join("active").exists());
        assert!(paths.extensions().join("gl-default").exists());
        assert!(paths.extensions().join("pinned-only").exists());
        assert!(!paths.extensions().join("unused").exists());
    }

    #[test]
    fn normal_app_uninstall_leaves_runtime_for_unused_cleanup() {
        let root = test_dir("uninstall-then-unused");
        let paths = Installation::for_test(&root);
        state::ensure_layout(&paths).unwrap();
        let app_id = "org.gnome.Calculator";
        let runtime_ref = "org.gnome.Platform/x86_64/50";
        let app_dir = paths.app(app_id).join("calculator-commit");
        let runtime_dir = paths
            .runtimes()
            .join("org.gnome.Platform-50")
            .join("runtime-commit");
        create_marked_checkout(
            &app_dir,
            "app/org.gnome.Calculator/x86_64/stable",
            "calculator-commit",
            &format!(
                "[Application]\nname={app_id}\nruntime={runtime_ref}\ncommand=gnome-calculator\n"
            ),
        );
        create_marked_checkout(
            &runtime_dir,
            &format!("runtime/{runtime_ref}"),
            "runtime-commit",
            "[Runtime]\nname=org.gnome.Platform\n",
        );
        state::record_install(
            &paths,
            &runtime::InstalledApp {
                app_id: app_id.to_string(),
                app_ref: "app/org.gnome.Calculator/x86_64/stable".to_string(),
                app_commit: "calculator-commit".to_string(),
                app_dir: app_dir.clone(),
                arch: "x86_64".to_string(),
                branch: "stable".to_string(),
                runtime_ref: runtime_ref.to_string(),
                runtime_commit: "runtime-commit".to_string(),
                runtime_dir: runtime_dir.clone(),
                command: "gnome-calculator".to_string(),
                timings: Default::default(),
            },
        )
        .unwrap();

        // This is the deployment-state transition performed by ordinary app
        // uninstall before repository refs and user-facing output are handled.
        let removed_app = state::remove_app_record(&paths, app_id).unwrap().unwrap();
        state::safe_remove_dir(&paths, &removed_app.app_dir).unwrap();
        state::cleanup_retired_deployments(&paths).unwrap();
        assert!(state::list_apps(&paths).unwrap().is_empty());
        assert!(state::get_runtime(&paths, runtime_ref).unwrap().is_some());
        assert!(runtime_dir.exists());

        let removed = remove_unused_deployment_checkouts(&paths).unwrap();
        assert_eq!(removed, vec![format!("runtime/{runtime_ref}")]);
        assert!(state::get_runtime(&paths, runtime_ref).unwrap().is_none());
        assert!(!runtime_dir.exists());
    }

    #[test]
    fn unused_cleanup_discovers_orphan_runtime_without_inventory_record() {
        let root = test_dir("unused-discovered-runtime");
        let paths = Installation::for_test(&root);
        state::ensure_layout(&paths).unwrap();
        let runtime_ref = "org.gnome.Platform/x86_64/50";
        let runtime_dir = paths
            .runtimes()
            .join("org.gnome.Platform-50")
            .join("runtime-commit");
        create_marked_checkout(
            &runtime_dir,
            &format!("runtime/{runtime_ref}"),
            "runtime-commit",
            "[Runtime]\nname=org.gnome.Platform\n",
        );
        assert!(state::list_runtimes(&paths).unwrap().is_empty());

        let removed = remove_unused_deployment_checkouts(&paths).unwrap();
        assert_eq!(removed, vec![format!("runtime/{runtime_ref}")]);
        assert!(!runtime_dir.exists());
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
    fn runtime_branch_change_reports_the_apps_previous_runtime_commit() {
        let root = test_dir("runtime-branch-reporting");
        let paths = Installation::for_test(&root);
        state::ensure_layout(&paths).unwrap();
        create_checkout(
            paths.data_root(),
            &PathBuf::from("apps").join("org.example.App"),
        );
        let runtime_50_dir = paths.runtimes().join("platform-50");
        create_checkout(paths.data_root(), &PathBuf::from("runtimes/platform-50"));
        state::write_runtime(
            &paths,
            &state::RuntimeRecord {
                runtime_ref: "org.example.Platform/x86_64/50".to_string(),
                runtime_commit: "runtime-50".to_string(),
                runtime_dir: paths.relative_data_path(&runtime_50_dir).unwrap(),
            },
        )
        .unwrap();

        let mut record = app_record(
            "org.example.App",
            "app/org.example.App/x86_64/stable",
            "app-1",
        );
        record.command = "new-command".to_string();
        record.runtime_ref = "org.example.Platform/x86_64/49".to_string();
        record.runtime_commit = "runtime-49".to_string();
        let mut remote = remote_app(&record.app_id, &record.app_ref, &record.app_commit);
        remote.runtime_ref = "org.example.Platform/x86_64/50".to_string();
        remote.runtime_commit = "runtime-50".to_string();

        let status = update_status(&paths, &record, &remote).unwrap();

        assert!(status.runtime_changed);
        assert!(!status.runtime_checkout_stale);
        assert_eq!(status.current_runtime_commit.as_deref(), Some("runtime-49"));
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
                transaction: TransactionOptions::default(),
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
