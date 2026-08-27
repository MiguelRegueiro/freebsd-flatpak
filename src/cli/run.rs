use crate::diagnostics::{Detail, Diagnostics};
use crate::installation as state;
use crate::installation::{self as runtime, installation_paths::Installation};
use crate::{desktop_integration, sandbox};
use anyhow::{Context, Result};
use sandbox::SandboxBackend;
use std::path::{Path, PathBuf};

pub(crate) fn cmd_run(
    paths: &Installation,
    args: Vec<String>,
    diagnostics: &Diagnostics,
) -> Result<()> {
    let (app_id, mut options) = parse_run_args(args)?;
    let app = diagnostics.measure(Detail::Summary, "run", "resolve app and runtime", || {
        if options.app_dir.is_none() && options.runtime_dir.is_none() && options.entry.is_none() {
            let record = state::get_app(paths, &app_id)?;
            options.app_dir = Some(state::absolute(paths, &record.app_dir));
            options.runtime_dir = Some(state::absolute(paths, &record.runtime_dir));
            options.entry = Some(record.command);
        }

        runtime::resolve_app(paths, &app_id, options)
    })?;

    let desktop = diagnostics.measure(Detail::Summary, "run", "desktop session", || {
        desktop_integration::DesktopSession::from_env()
            .context("XDG_RUNTIME_DIR and WAYLAND_DISPLAY must be set")
    })?;
    let backend = sandbox::ChrootNullfsBackend::new(paths.clone());
    let status = backend.run(&app, &desktop, diagnostics)?;
    if !status.success() {
        anyhow::bail!("{} exited with status {}", app.app_id, status);
    }
    Ok(())
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
