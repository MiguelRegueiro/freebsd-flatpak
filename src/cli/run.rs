use crate::diagnostics::{Detail, Diagnostics};
use crate::flatpak_ref::{FlatpakRef, PartialRef, RefKind};
use crate::installation as state;
use crate::installation::{self as runtime, installation_paths::Installation};
use crate::{desktop_integration, sandbox};
use anyhow::{bail, Context, Result};
use sandbox::SandboxBackend;
use std::path::{Path, PathBuf};

pub(crate) fn cmd_run(
    paths: &Installation,
    args: Vec<String>,
    diagnostics: &Diagnostics,
) -> Result<()> {
    let parsed = parse_run_args(args)?;
    let app_id = parsed.app_id;
    let mut options = parsed.resolve;
    let app = diagnostics.measure(Detail::Summary, "run", "resolve app and runtime", || {
        if parsed.runtime.is_some() || parsed.runtime_version.is_some() {
            let record = state::get_app(paths, &app_id)?;
            options
                .app_dir
                .get_or_insert_with(|| state::absolute(paths, &record.app_dir));
            options.entry.get_or_insert(record.command);
            let runtime = resolve_runtime_override(
                paths,
                &record.runtime_ref,
                parsed.runtime.as_deref(),
                parsed.runtime_version.as_deref(),
            )?;
            options.runtime_ref = Some(runtime.runtime_ref);
            options.runtime_dir = Some(state::absolute(paths, &runtime.runtime_dir));
        } else if options.app_dir.is_none()
            && options.runtime_dir.is_none()
            && options.entry.is_none()
        {
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
#[derive(Debug)]
struct RunOptions {
    app_id: String,
    resolve: runtime::ResolveAppOptions,
    runtime: Option<String>,
    runtime_version: Option<String>,
}

fn parse_run_args(args: Vec<String>) -> Result<RunOptions> {
    let mut args = args.into_iter();
    let mut app_id = None;
    let mut options = runtime::ResolveAppOptions::default();
    let mut runtime = None;
    let mut runtime_version = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--" => {
                if app_id.is_none() {
                    app_id = Some(args.next().context("missing app id")?);
                }
                options.args.extend(args);
                break;
            }
            "--app-dir" => options.app_dir = Some(next_path(&mut args, "--app-dir")?),
            "--runtime-dir" => options.runtime_dir = Some(next_path(&mut args, "--runtime-dir")?),
            "--entry" => options.entry = Some(args.next().context("missing value for --entry")?),
            "--runtime" => set_once(&mut runtime, args.next(), "--runtime")?,
            "--runtime-version" => {
                set_once(&mut runtime_version, args.next(), "--runtime-version")?
            }
            _ if arg.starts_with("--app-dir=") => {
                options.app_dir = Some(PathBuf::from(value_after_equals(&arg)))
            }
            _ if arg.starts_with("--runtime-dir=") => {
                options.runtime_dir = Some(PathBuf::from(value_after_equals(&arg)))
            }
            _ if arg.starts_with("--entry=") => {
                options.entry = Some(value_after_equals(&arg).to_string())
            }
            _ if arg.starts_with("--runtime=") => {
                set_once(
                    &mut runtime,
                    Some(value_after_equals(&arg).to_string()),
                    "--runtime",
                )?;
            }
            _ if arg.starts_with("--runtime-version=") => {
                set_once(
                    &mut runtime_version,
                    Some(value_after_equals(&arg).to_string()),
                    "--runtime-version",
                )?;
            }
            _ if app_id.is_none() && arg.starts_with('-') => bail!("unknown run option: {arg}"),
            _ if app_id.is_none() => app_id = Some(arg),
            _ => {
                options.args.push(arg);
                options.args.extend(args);
                break;
            }
        }
    }

    Ok(RunOptions {
        app_id: app_id.context("missing app id")?,
        resolve: options,
        runtime,
        runtime_version,
    })
}

fn set_once(target: &mut Option<String>, value: Option<String>, option: &str) -> Result<()> {
    if target.is_some() {
        bail!("{option} may only be specified once");
    }
    *target = Some(value.with_context(|| format!("missing value for {option}"))?);
    Ok(())
}

fn resolve_runtime_override(
    paths: &Installation,
    current_runtime: &str,
    requested: Option<&str>,
    requested_version: Option<&str>,
) -> Result<state::RuntimeRecord> {
    let current = FlatpakRef::parse(&format!("runtime/{current_runtime}"))?;
    let requested = requested.map(PartialRef::parse).transpose()?;
    if let Some(requested) = &requested {
        requested.effective_kind(Some(RefKind::Runtime))?;
    }
    let id = requested
        .as_ref()
        .map_or(current.id.as_str(), |requested| requested.id.as_str());
    let arch = requested
        .as_ref()
        .and_then(|requested| requested.arch.as_deref())
        .unwrap_or(&current.arch);
    let branch = requested_version
        .or_else(|| {
            requested
                .as_ref()
                .and_then(|requested| requested.branch.as_deref())
        })
        .unwrap_or(&current.branch);
    let runtime_ref = FlatpakRef::parse(&format!("runtime/{id}/{arch}/{branch}"))?.partial_ref();
    state::get_runtime(paths, &runtime_ref)?
        .with_context(|| format!("runtime/{runtime_ref} is not installed"))
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

#[cfg(test)]
#[path = "tests/run.rs"]
mod tests;
