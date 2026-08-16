mod desktop;
mod linuxulator;
mod runtime;
mod sandbox;

use anyhow::{Context, Result};
use sandbox::SandboxBackend;
use std::path::{Path, PathBuf};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("checkout") => {
            let ref_name = args.next().context("missing ref")?;
            let dest = args.next().context("missing destination")?;
            runtime::checkout_ref(&ref_name, PathBuf::from(dest))
        }
        Some("inspect") => {
            let refs: Vec<String> = args.collect();
            runtime::inspect_refs(&refs)
        }
        Some("install") => {
            let project_root = std::env::current_dir().context("determine project root")?;
            let app_id = args.next().context("missing app id")?;
            if let Some(extra) = args.next() {
                anyhow::bail!("unexpected install argument: {extra}");
            }
            let installed = runtime::install_app(&project_root, &app_id)?;
            println!("installed {}", installed.app_id);
            println!("  app ref: {}", installed.app_ref);
            println!("  app commit: {}", installed.app_commit);
            println!("  app dir: {}", installed.app_dir.display());
            println!("  arch: {}", installed.arch);
            println!("  branch: {}", installed.branch);
            println!("  runtime: {}", installed.runtime_ref);
            println!("  runtime commit: {}", installed.runtime_commit);
            println!("  runtime dir: {}", installed.runtime_dir.display());
            println!("  command: {}", installed.command);
            Ok(())
        }
        Some("run") => {
            let project_root = std::env::current_dir().context("determine project root")?;
            let (app_id, options) = parse_run_args(args.collect())?;
            let app = runtime::resolve_app(&project_root, &app_id, options)?;
            let desktop = desktop::DesktopSession::from_env()
                .context("XDG_RUNTIME_DIR and WAYLAND_DISPLAY must be set")?;
            let backend = sandbox::ChrootNullfsBackend::new(project_root);
            let status = backend.run(&app, &desktop)?;
            if !status.success() {
                anyhow::bail!("{} exited with status {}", app.app_id, status);
            }
            Ok(())
        }
        Some(cmd) => anyhow::bail!("unknown command: {cmd}"),
        None => {
            eprintln!("usage:");
            eprintln!("  freebsd-flatpak-poc inspect");
            eprintln!("  freebsd-flatpak-poc inspect <ostree-ref>...");
            eprintln!("  freebsd-flatpak-poc checkout <ostree-ref> <destination>");
            eprintln!("  freebsd-flatpak-poc install <app-id>");
            eprintln!("  freebsd-flatpak-poc run <app-id> [--app-dir PATH] [--runtime-dir PATH] [--entry EXECUTABLE]");
            Ok(())
        }
    }
}

fn parse_run_args(args: Vec<String>) -> Result<(String, runtime::ResolveAppOptions)> {
    let mut args = args.into_iter();
    let app_id = args.next().context("missing app id")?;
    let mut options = runtime::ResolveAppOptions::default();

    while let Some(arg) = args.next() {
        match arg.as_str() {
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
            _ => anyhow::bail!("unknown run option: {arg}"),
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
