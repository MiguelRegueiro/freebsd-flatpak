mod audio;
mod cursor;
mod desktop;
mod filesystem;
mod fonts;
mod graphics;
mod linuxulator;
mod portal;
mod runtime;
mod sandbox;
mod state;
mod video;

use anyhow::{bail, Context, Result};
use sandbox::SandboxBackend;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn main() -> Result<()> {
    let project_root = find_project_root()?;
    std::env::set_current_dir(&project_root)
        .with_context(|| format!("enter project root {}", project_root.display()))?;
    state::ensure_layout(&project_root)?;
    sandbox::recover_stale_mounts(&project_root)?;
    portal::recover_stale_portal_mounts(&project_root)?;
    graphics::recover_stale_graphics_dirs(&project_root)?;

    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("search") => cmd_search(args.collect()),
        Some("install") => cmd_install(&project_root, args.collect()),
        Some("list") => cmd_list(&project_root, args.collect()),
        Some("permissions") => cmd_permissions(&project_root, args.collect()),
        Some("run") => cmd_run(&project_root, args.collect()),
        Some("uninstall") => cmd_uninstall(&project_root, args.collect()),
        Some("update") => cmd_update(&project_root, args.collect()),
        Some("checkout") => {
            let ref_name = args.next().context("missing ref")?;
            let dest = args.next().context("missing destination")?;
            runtime::checkout_ref(&ref_name, PathBuf::from(dest))
        }
        Some("inspect") => {
            let refs: Vec<String> = args.collect();
            runtime::inspect_refs(&refs)
        }
        Some(cmd) => anyhow::bail!("unknown command: {cmd}"),
        None => {
            print_usage();
            Ok(())
        }
    }
}

fn cmd_search(args: Vec<String>) -> Result<()> {
    if args.len() != 1 {
        bail!("usage: flatpak search <query>");
    }
    let results = runtime::search_apps(&args[0])?;
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

fn cmd_install(project_root: &Path, args: Vec<String>) -> Result<()> {
    if args.len() != 1 {
        bail!("usage: flatpak install <app-id>");
    }
    let installed = runtime::install_app(project_root, &args[0])?;
    let record = state::record_install(project_root, &installed)?;
    let export = desktop::export_app(project_root, &record)?;
    println!("Installed {}", installed.app_id);
    println!("  app ref: {}", installed.app_ref);
    println!("  app commit: {}", installed.app_commit);
    println!("  runtime: {}", installed.runtime_ref);
    println!("  runtime commit: {}", installed.runtime_commit);
    println!("  command: {}", installed.command);
    print_export_report(project_root, &export);
    Ok(())
}

fn cmd_list(project_root: &Path, args: Vec<String>) -> Result<()> {
    if !args.is_empty() {
        bail!("usage: flatpak list");
    }
    let apps = state::list_apps(project_root)?;
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

fn cmd_permissions(project_root: &Path, args: Vec<String>) -> Result<()> {
    if args.len() != 1 {
        bail!("usage: flatpak permissions <app-id>");
    }
    let record = state::get_app(project_root, &args[0])?;
    let user = std::env::var("USER").unwrap_or_else(|_| "user".to_string());
    let sandbox_root = project_root
        .join("runtime")
        .join("chroots")
        .join(&record.app_id);
    let uid = numeric_id("id", "-u")?;
    let xdg_runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{uid}")));
    let metadata_path = state::absolute(project_root, &record.app_dir).join("metadata");
    let host_filesystem = filesystem::HostFilesystem::from_metadata_file_for_user(
        &metadata_path,
        &user,
        project_root,
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

fn cmd_run(project_root: &Path, args: Vec<String>) -> Result<()> {
    let (app_id, mut options) = parse_run_args(args)?;
    if options.app_dir.is_none() && options.runtime_dir.is_none() && options.entry.is_none() {
        let record = state::get_app(project_root, &app_id)?;
        options.app_dir = Some(state::absolute(project_root, &record.app_dir));
        options.runtime_dir = Some(state::absolute(project_root, &record.runtime_dir));
        options.entry = Some(record.command);
    }

    let app = runtime::resolve_app(project_root, &app_id, options)?;
    let desktop = desktop::DesktopSession::from_env()
        .context("XDG_RUNTIME_DIR and WAYLAND_DISPLAY must be set")?;
    let backend = sandbox::ChrootNullfsBackend::new(project_root.to_path_buf());
    let status = backend.run(&app, &desktop)?;
    if !status.success() {
        anyhow::bail!("{} exited with status {}", app.app_id, status);
    }
    Ok(())
}

fn cmd_uninstall(project_root: &Path, args: Vec<String>) -> Result<()> {
    if args.len() != 1 {
        bail!("usage: flatpak uninstall <app-id>");
    }
    let app_id = &args[0];
    if sandbox::app_has_mounts(project_root, app_id)? {
        bail!("{app_id} still has active sandbox mounts; stop it before uninstalling");
    }
    let record = match state::get_app(project_root, app_id) {
        Ok(record) => record,
        Err(_) => {
            desktop::remove_export(project_root, app_id)?;
            println!("{app_id} is not installed");
            return Ok(());
        }
    };
    desktop::remove_export(project_root, app_id)?;
    if state::remove_app_record(project_root, app_id)?.is_none() {
        println!("{app_id} is not installed");
        return Ok(());
    }

    state::safe_remove_dir(project_root, &record.app_dir)?;
    let chroot_dir = project_root.join("runtime").join("chroots").join(app_id);
    state::safe_remove_dir(project_root, &chroot_dir)?;

    if state::runtime_is_required(project_root, &record.runtime_ref)? {
        println!("Uninstalled {app_id}");
        println!("  kept shared runtime {}", record.runtime_ref);
    } else {
        state::safe_remove_dir(project_root, &record.runtime_dir)?;
        state::remove_runtime_record(project_root, &record.runtime_ref)?;
        println!("Uninstalled {app_id}");
        println!("  removed unused runtime {}", record.runtime_ref);
    }
    Ok(())
}

fn cmd_update(project_root: &Path, args: Vec<String>) -> Result<()> {
    let targets = update_targets(project_root, args)?;

    if targets.is_empty() {
        println!("No installed apps");
        return Ok(());
    }

    let mut touched_runtimes = BTreeSet::new();
    for target in targets {
        let record = target.record;
        if sandbox::app_has_mounts(project_root, &record.app_id)? {
            bail!(
                "{} still has active sandbox mounts; stop it before updating",
                record.app_id
            );
        }

        let remote = match target.remote {
            Some(remote) => remote,
            None => runtime::resolve_remote_app(&record.app_id)?,
        };
        let status = update_status(project_root, &record, &remote)?;

        if !status.app_changed && !status.runtime_changed {
            println!("{} is up to date", record.app_id);
            let export = desktop::export_app(project_root, &record)?;
            print_export_report(project_root, &export);
            continue;
        }

        let force_runtime =
            status.runtime_checkout_stale && touched_runtimes.insert(remote.runtime_ref.clone());
        let installed = runtime::update_app(
            project_root,
            &remote,
            status.app_checkout_stale,
            force_runtime,
        )?;
        let installed_record = state::record_install(project_root, &installed)?;
        if record.app_id != installed.app_id {
            desktop::remove_export(project_root, &record.app_id)?;
            state::remove_app_record(project_root, &record.app_id)?;
            state::safe_remove_dir(project_root, &record.app_dir)?;
        }
        let export = desktop::export_app(project_root, &installed_record)?;
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
        print_export_report(project_root, &export);

        if record.runtime_ref != installed.runtime_ref
            && !state::runtime_is_required(project_root, &record.runtime_ref)?
        {
            state::safe_remove_dir(project_root, &record.runtime_dir)?;
            state::remove_runtime_record(project_root, &record.runtime_ref)?;
        }
    }

    Ok(())
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

fn update_targets(project_root: &Path, args: Vec<String>) -> Result<Vec<UpdateTarget>> {
    let installed = state::list_apps(project_root)?;
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
            let remote = runtime::resolve_remote_app(&requested)
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
    project_root: &Path,
    record: &state::AppRecord,
    remote: &runtime::RemoteApp,
) -> Result<UpdateStatus> {
    let app_dir = state::absolute(project_root, &record.app_dir);
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

    let runtime_dir = project_root
        .join("runtime")
        .join(runtime::runtime_checkout_dir(&remote.runtime_ref));
    let current_runtime_commit =
        state::runtime_commit(project_root, &remote.runtime_ref)?.or_else(|| {
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

fn print_export_report(project_root: &Path, export: &desktop::ExportReport) {
    println!("  exported files: {}", export.files);
    println!("  desktop entries: {}", export.desktop_entries);
    println!(
        "  export data dir: {}",
        desktop::export_data_dir(project_root).display()
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

fn find_project_root() -> Result<PathBuf> {
    if let Ok(root) = std::env::var("FREEBSD_FLATPAK_POC_ROOT") {
        return Ok(PathBuf::from(root));
    }

    let cwd = std::env::current_dir().context("determine current directory")?;
    if looks_like_project_root(&cwd) {
        return Ok(cwd);
    }

    let exe = std::env::current_exe().context("determine executable path")?;
    if let Some(parent) = exe.parent() {
        if parent.file_name().and_then(|name| name.to_str()) == Some("bin") {
            if let Some(root) = parent.parent() {
                if looks_like_project_root(root) {
                    return Ok(root.to_path_buf());
                }
            }
        }
        if parent.file_name().and_then(|name| name.to_str()) == Some("debug") {
            if let Some(target) = parent.parent() {
                if target.file_name().and_then(|name| name.to_str()) == Some("target") {
                    if let Some(root) = target.parent() {
                        if looks_like_project_root(root) {
                            return Ok(root.to_path_buf());
                        }
                    }
                }
            }
        }
    }

    bail!(
        "could not find freebsd-flatpak-poc project root; run from the project or set FREEBSD_FLATPAK_POC_ROOT"
    );
}

fn looks_like_project_root(path: &Path) -> bool {
    path.join("Cargo.toml").is_file() && path.join("runtime").is_dir()
}

fn print_usage() {
    eprintln!("usage:");
    eprintln!("  flatpak search <query>");
    eprintln!("  flatpak install <app-id>");
    eprintln!("  flatpak list");
    eprintln!("  flatpak permissions <app-id>");
    eprintln!("  flatpak run <app-id> [-- app-args...]");
    eprintln!("  flatpak uninstall <app-id>");
    eprintln!("  flatpak update [app-id...]");
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
            app_dir: PathBuf::from("runtime").join("app").join(app_id),
            arch: "x86_64".to_string(),
            branch: "stable".to_string(),
            runtime_ref: "org.example.Platform/x86_64/stable".to_string(),
            runtime_commit: "runtime-1".to_string(),
            runtime_dir: PathBuf::from("runtime").join("org.example.Platform-stable"),
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
            command: "new-command".to_string(),
        }
    }

    fn create_runtime_checkout(root: &Path) {
        create_checkout(
            root,
            &PathBuf::from("runtime").join("org.example.Platform-stable"),
        );
    }

    #[test]
    fn newer_remote_app_commit_requires_app_checkout() {
        let root = test_dir("newer-app-commit");
        create_checkout(
            &root,
            &PathBuf::from("runtime").join("app").join("org.example.App"),
        );
        create_runtime_checkout(&root);
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

        let status = update_status(&root, &record, &remote).unwrap();

        assert!(status.app_changed);
        assert!(status.app_checkout_stale);
        assert!(!status.runtime_changed);
        assert!(!status.runtime_checkout_stale);
    }

    #[test]
    fn app_id_replacement_requires_app_checkout_even_with_same_commit() {
        let root = test_dir("replacement");
        create_checkout(
            &root,
            &PathBuf::from("runtime")
                .join("app")
                .join("org.example.OldApp"),
        );
        create_runtime_checkout(&root);
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

        let status = update_status(&root, &record, &remote).unwrap();

        assert!(status.app_changed);
        assert!(status.app_checkout_stale);
    }

    #[test]
    fn missing_runtime_checkout_requires_runtime_checkout() {
        let root = test_dir("missing-runtime");
        create_checkout(
            &root,
            &PathBuf::from("runtime").join("app").join("org.example.App"),
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

        let status = update_status(&root, &record, &remote).unwrap();

        assert!(status.runtime_changed);
        assert!(status.runtime_checkout_stale);
        assert_eq!(status.current_runtime_commit.as_deref(), Some("runtime-1"));
    }

    #[test]
    fn stale_record_command_updates_state_without_app_checkout() {
        let root = test_dir("state-only-command");
        create_checkout(
            &root,
            &PathBuf::from("runtime").join("app").join("org.example.App"),
        );
        create_runtime_checkout(&root);
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

        let status = update_status(&root, &record, &remote).unwrap();

        assert!(status.app_changed);
        assert!(!status.app_checkout_stale);
    }
}
